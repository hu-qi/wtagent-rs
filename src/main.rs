use std::{ffi::OsString, path::PathBuf, sync::Arc, time::Duration};

use clap::{CommandFactory, Parser, Subcommand};
use tracing_subscriber::EnvFilter;
use wtagent_rs::{
    browser::{
        adapter::{BrowserWebAdapter, WebAdapter},
        backend::{resolve_browser_backend, BrowserBackend},
        chrome::discover_chrome,
        ego::{discover_ego, EgoClient},
        provider::{ProviderConfig, ProviderId},
        throttle::RateController,
    },
    config::{default_app_data_dir, AppConfig, ApprovalMode},
    policy::PolicyEngine,
    runtime::{AgentRuntime, TerminalApproval},
    session::{list_sessions, SessionStore},
    tools::ToolExecutor,
    Result, WtError,
};

#[derive(Debug, Parser)]
#[command(
    name = "wtagent",
    version,
    about = "Use your web AI account as a local Rust coding agent",
    subcommand_precedence_over_arg = true
)]
struct Cli {
    /// Web AI provider. Defaults to ChatGPT for new sessions; resume keeps the saved provider.
    #[arg(long, value_enum, global = true)]
    model: Option<ProviderId>,

    /// Provider-specific mode, e.g. ChatGPT pro/current. Providers without a switcher keep their current mode.
    #[arg(long, global = true)]
    mode: Option<String>,

    /// Project directory.
    #[arg(long, short = 'C', default_value = ".", global = true)]
    project: PathBuf,

    /// Explicit Chrome/Chromium executable. Supplying this forces the Chrome backend.
    #[arg(long, global = true)]
    chrome_path: Option<PathBuf>,

    /// Ask, auto, or read-only local side-effect policy.
    #[arg(long, value_enum, default_value = "ask", global = true)]
    approval: ApprovalMode,

    /// Start Chrome minimized where the platform supports it.
    #[arg(long, global = true)]
    minimized: bool,

    /// Minimum delay between outbound provider messages.
    #[arg(long, default_value_t = 4_000, global = true)]
    min_send_interval_ms: u64,

    /// Maximum outbound provider messages per rolling minute.
    #[arg(long, default_value_t = 6, global = true)]
    max_sends_per_minute: usize,

    /// Enable verbose diagnostics.
    #[arg(long, global = true)]
    debug: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Run a new task. `wtagent "task"` is accepted as shorthand.
    Run {
        #[arg(required = true, trailing_var_arg = true)]
        task: Vec<String>,
        /// Attach a file to the initial web message (provider support varies).
        #[arg(long = "file", short = 'f')]
        files: Vec<PathBuf>,
    },
    /// Resume a saved session and optionally add a follow-up instruction.
    Resume {
        session_id: String,
        #[arg(trailing_var_arg = true)]
        instruction: Vec<String>,
    },
    /// Open the selected provider browser and wait for manual login when required.
    Login,
    /// Manage ego-lite Task Space ownership.
    Ego {
        #[command(subcommand)]
        command: EgoCommands,
    },
    /// Check browser backend, project paths, provider metadata, and data directories.
    Doctor,
    /// List supported providers and their web endpoints.
    Providers,
    /// List recent saved sessions.
    Sessions {
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
}

#[derive(Debug, Subcommand)]
enum EgoCommands {
    /// Explicitly return the provider Task Space from user control to WTAgent-RS.
    Claim,
}

#[tokio::main]
async fn main() {
    let cli = parse_cli_with_bare_task();
    init_tracing(cli.debug);
    if let Err(error) = run(cli).await {
        eprintln!("error: {error}");
        match error {
            WtError::UsageLimit(_) | WtError::RateLimit(_) => {
                eprintln!(
                    "WTAgent-RS will not retry around a provider limit. Wait for the provider to recover/reset, then resume the saved session."
                );
            }
            WtError::Challenge(_) => {
                eprintln!(
                    "Complete the challenge manually in the active browser; WTAgent-RS does not bypass CAPTCHAs or anti-bot checks."
                );
            }
            _ => {}
        }
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<()> {
    match cli.command.as_ref() {
        Some(Commands::Providers) => {
            print_providers();
            Ok(())
        }
        Some(Commands::Sessions { limit }) => {
            for state in list_sessions(&default_app_data_dir()?, *limit).await? {
                println!(
                    "{}  {:<9} turn={:<3} phase={:<12} {}",
                    state.session_id,
                    format!("{:?}", state.provider).to_lowercase(),
                    state.turn,
                    state.phase,
                    state.task.lines().next().unwrap_or_default()
                );
            }
            Ok(())
        }
        Some(Commands::Doctor) => doctor(&cli).await,
        Some(Commands::Login) => login(&cli).await,
        Some(Commands::Ego { command }) => ego_command(&cli, command).await,
        Some(Commands::Run { task, files }) => run_new(&cli, task.join(" "), files.clone()).await,
        Some(Commands::Resume {
            session_id,
            instruction,
        }) => resume(&cli, session_id, instruction.join(" ")).await,
        None => {
            Cli::command().print_help().map_err(WtError::Io)?;
            println!();
            Ok(())
        }
    }
}

async fn run_new(cli: &Cli, task: String, files: Vec<PathBuf>) -> Result<()> {
    let provider = cli.model.unwrap_or_default();
    let config = configured(cli, provider, cli.project.clone())?;
    validate_files(&files)?;
    let session =
        SessionStore::create(&config.app_data_dir, provider, &config.project_root, task).await?;
    let session_id = session.state.session_id.clone();
    eprintln!("session: {session_id}");
    let runtime = runtime_from(config, session);
    let result = runtime.run(false, None, files, cli.mode.clone()).await?;
    println!("{result}");
    Ok(())
}

async fn resume(cli: &Cli, session_id: &str, instruction: String) -> Result<()> {
    let app_data = default_app_data_dir()?;
    let session = SessionStore::load(&app_data, session_id).await?;
    let provider = cli.model.unwrap_or(session.state.provider);
    if provider != session.state.provider {
        return Err(WtError::Config(format!(
            "session was created with {:?}; provider migration is intentionally not automatic",
            session.state.provider
        )));
    }
    let config = configured(cli, provider, session.state.project_root.clone())?;
    let runtime = runtime_from(config, session);
    let instruction = (!instruction.trim().is_empty()).then_some(instruction);
    let result = runtime
        .run(true, instruction, Vec::new(), cli.mode.clone())
        .await?;
    println!("{result}");
    Ok(())
}

fn runtime_from(config: AppConfig, session: SessionStore) -> AgentRuntime {
    let adapter: Box<dyn WebAdapter> = Box::new(BrowserWebAdapter::new(
        config.provider,
        config.profile_dir(),
        config.chrome_path.clone(),
        config.minimized,
    ));
    let policy = PolicyEngine::new(config.project_root.clone(), config.approval);
    let tools = ToolExecutor::new(policy, config.limits.clone());
    let rate = RateController::new(config.rate.clone());
    AgentRuntime::new(
        adapter,
        tools,
        session,
        rate,
        Arc::new(TerminalApproval),
        config.limits,
    )
}

fn configured(cli: &Cli, provider: ProviderId, project: PathBuf) -> Result<AppConfig> {
    let mut config = AppConfig::new(provider, project)?;
    config.mode = cli.mode.clone();
    config.chrome_path = cli.chrome_path.clone();
    config.minimized = cli.minimized;
    config.approval = cli.approval;
    config.rate.min_send_interval = Duration::from_millis(cli.min_send_interval_ms);
    config.rate.max_sends_per_minute = cli.max_sends_per_minute.max(1);
    Ok(config)
}

async fn login(cli: &Cli) -> Result<()> {
    let provider = cli.model.unwrap_or_default();
    let config = configured(cli, provider, cli.project.clone())?;
    let mut adapter = BrowserWebAdapter::new(
        provider,
        config.profile_dir(),
        config.chrome_path.clone(),
        false,
    );
    adapter.launch(None).await?;
    if adapter.auth_state().await? == wtagent_rs::browser::adapter::AuthState::Authenticated {
        println!("{} is already signed in.", provider.label());
        return Ok(());
    }
    eprintln!(
        "Sign in manually in the active browser. WTAgent-RS does not automate credentials or challenges."
    );
    adapter
        .wait_for_manual_login(Duration::from_secs(10 * 60))
        .await?;
    adapter.start_conversation(None).await?;
    println!("{} login detected.", provider.label());
    Ok(())
}

async fn ego_command(cli: &Cli, command: &EgoCommands) -> Result<()> {
    let provider = cli.model.unwrap_or_default();
    match command {
        EgoCommands::Claim => {
            if cli.chrome_path.is_some() {
                return Err(WtError::Config(
                    "`wtagent ego claim` is only valid for the ego-lite backend; remove --chrome-path"
                        .into(),
                ));
            }
            let task_space = ego_task_space(provider);
            EgoClient::claim_task_space(None, task_space.clone()).await?;
            println!(
                "ego-lite task space `{task_space}` is now controlled by WTAgent-RS. Retry or resume your task."
            );
            Ok(())
        }
    }
}

async fn doctor(cli: &Cli) -> Result<()> {
    let provider = cli.model.unwrap_or_default();
    let config = configured(cli, provider, cli.project.clone())?;
    println!("WTAgent-RS doctor");
    println!("  project: {}", config.project_root.display());
    println!(
        "  provider: {} ({})",
        provider.label(),
        provider.config().base_url
    );
    println!("  data dir: {}", config.app_data_dir.display());
    println!("  profile: {}", config.profile_dir().display());
    tokio::fs::create_dir_all(&config.app_data_dir).await?;
    let backend =
        resolve_browser_backend(BrowserBackend::Auto, config.chrome_path.as_deref(), None)?;
    println!("  browser backend: {backend}");
    match backend {
        BrowserBackend::Chrome => {
            let chrome = discover_chrome(config.chrome_path.as_deref())?;
            println!("  chrome: {}", chrome.display());
        }
        BrowserBackend::Ego => {
            let ego = discover_ego(None)?;
            println!("  ego-browser: {}", ego.display());
            println!("  ego task space: {}", ego_task_space(provider));
        }
        BrowserBackend::Auto => unreachable!("doctor resolves auto before reporting"),
    }
    println!(
        "  rate policy: min={}ms, max={}/minute",
        cli.min_send_interval_ms, cli.max_sends_per_minute
    );
    println!("  anti-bot policy: manual challenge only; no CAPTCHA bypass/fingerprint spoofing/account rotation");
    println!("status: OK");
    Ok(())
}

fn ego_task_space(provider: ProviderId) -> String {
    format!(
        "wtagent-rs-{}",
        format!("{provider:?}").to_ascii_lowercase()
    )
}

fn print_providers() {
    for provider in ProviderConfig::all() {
        println!(
            "{:<9} {:<10} {}  default-mode={}",
            format!("{:?}", provider.id).to_lowercase(),
            provider.label,
            provider.base_url,
            provider.default_mode.unwrap_or("site-current")
        );
    }
}

fn validate_files(files: &[PathBuf]) -> Result<()> {
    for path in files {
        if !path.is_file() {
            return Err(WtError::Config(format!(
                "attachment does not exist or is not a file: {}",
                path.display()
            )));
        }
    }
    Ok(())
}

fn init_tracing(debug: bool) {
    let default = if debug {
        "wtagent_rs=debug"
    } else {
        "wtagent_rs=warn"
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}

fn parse_cli_with_bare_task() -> Cli {
    let args: Vec<OsString> = std::env::args_os().collect();
    match Cli::try_parse_from(args.clone()) {
        Ok(cli) => cli,
        Err(first) => {
            // Preserve the original WTAgent shorthand: `wtagent "do X"`.
            // A second parse with an injected `run` is attempted only after the
            // normal subcommand grammar failed; if it also fails, show Clap's
            // original-quality error and exit.
            let mut fallback = args;
            if fallback.len() > 1 {
                fallback.insert(1, OsString::from("run"));
                if let Ok(cli) = Cli::try_parse_from(fallback) {
                    return cli;
                }
            }
            first.exit();
        }
    }
}
