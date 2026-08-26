use std::{ffi::OsString, path::PathBuf, sync::Arc, time::Duration};

use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
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
    chatgpt_project::{list_chatgpt_projects, resolve_chatgpt_project},
    config::{default_app_data_dir, AppConfig, ApprovalMode},
    policy::PolicyEngine,
    runtime::{AgentRuntime, TerminalApproval},
    session::{
        delete_session, latest_session_for_project, list_sessions, SessionState, SessionStore,
    },
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

    /// Create a new ChatGPT task inside this Project. Accepts an exact Project name or Project URL.
    #[arg(long, global = true, value_name = "NAME_OR_URL")]
    chatgpt_project: Option<String>,

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
    /// Manage persistent WTAgent sessions.
    Session {
        #[command(subcommand)]
        command: Option<SessionCommands>,
    },
    /// Legacy alias for `wtagent session list`.
    Sessions {
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Inspect ChatGPT-specific resources.
    Chatgpt {
        #[command(subcommand)]
        command: ChatGptCommands,
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
}

#[derive(Debug, Subcommand)]
enum SessionCommands {
    /// List recent sessions.
    List {
        #[arg(long, short = 'n', default_value_t = 20)]
        limit: usize,
        #[arg(long, value_enum, default_value_t = SessionOutputFormat::Table)]
        format: SessionOutputFormat,
    },
    /// Show one session in detail.
    Show {
        session_id: String,
        #[arg(long, value_enum, default_value_t = SessionOutputFormat::Table)]
        format: SessionOutputFormat,
    },
    /// Resume a specific session.
    Resume {
        session_id: String,
        #[arg(trailing_var_arg = true)]
        instruction: Vec<String>,
    },
    /// Continue the most recently updated session for the current project.
    Continue {
        #[arg(trailing_var_arg = true)]
        instruction: Vec<String>,
    },
    /// Delete a session and its local event/state data.
    Delete { session_id: String },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum SessionOutputFormat {
    Table,
    Json,
}

#[derive(Debug, Subcommand)]
enum ChatGptCommands {
    /// List ChatGPT Projects visible in the authenticated browser session.
    Projects {
        #[arg(long, value_enum, default_value_t = SessionOutputFormat::Table)]
        format: SessionOutputFormat,
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
    if cli.debug {
        eprintln!(
            "debug: wtagent version={} pid={} binary={}",
            env!("CARGO_PKG_VERSION"),
            std::process::id(),
            current_executable_display()
        );
    }
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
        Some(Commands::Session { command }) => session_command(&cli, command.as_ref()).await,
        Some(Commands::Sessions { limit }) => {
            eprintln!("note: `wtagent sessions` is deprecated; use `wtagent session list`");
            print_session_list(*limit, SessionOutputFormat::Table).await
        }
        Some(Commands::Chatgpt { command }) => chatgpt_command(&cli, command).await,
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

async fn session_command(cli: &Cli, command: Option<&SessionCommands>) -> Result<()> {
    match command {
        None => print_session_list(20, SessionOutputFormat::Table).await,
        Some(SessionCommands::List { limit, format }) => print_session_list(*limit, *format).await,
        Some(SessionCommands::Show { session_id, format }) => {
            let store = SessionStore::load(&default_app_data_dir()?, session_id).await?;
            print_session(&store.state, *format)?;
            Ok(())
        }
        Some(SessionCommands::Resume {
            session_id,
            instruction,
        }) => resume(cli, session_id, instruction.join(" ")).await,
        Some(SessionCommands::Continue { instruction }) => {
            let project_config =
                configured(cli, cli.model.unwrap_or_default(), cli.project.clone())?;
            let Some(state) = latest_session_for_project(
                &project_config.app_data_dir,
                &project_config.project_root,
            )
            .await?
            else {
                return Err(WtError::Session(format!(
                    "no saved session found for project {}",
                    project_config.project_root.display()
                )));
            };
            eprintln!("continuing session: {}", state.session_id);
            resume(cli, &state.session_id, instruction.join(" ")).await
        }
        Some(SessionCommands::Delete { session_id }) => {
            delete_session(&default_app_data_dir()?, session_id).await?;
            println!("deleted session: {session_id}");
            Ok(())
        }
    }
}

async fn chatgpt_command(cli: &Cli, command: &ChatGptCommands) -> Result<()> {
    let config = configured(cli, ProviderId::Chatgpt, cli.project.clone())?;
    match command {
        ChatGptCommands::Projects { format } => {
            let projects = list_chatgpt_projects(&config).await?;
            match format {
                SessionOutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&projects)?);
                }
                SessionOutputFormat::Table => {
                    if projects.is_empty() {
                        println!("No ChatGPT Projects were found in the authenticated account.");
                        return Ok(());
                    }
                    println!("PROJECT ID                                      NAME / URL");
                    for project in projects {
                        println!(
                            "{:<47} {} / {}",
                            project.project_id,
                            project.name.as_deref().unwrap_or("<unnamed>"),
                            project.url
                        );
                    }
                }
            }
            Ok(())
        }
    }
}

async fn print_session_list(limit: usize, format: SessionOutputFormat) -> Result<()> {
    let sessions = list_sessions(&default_app_data_dir()?, limit).await?;
    match format {
        SessionOutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&sessions)?);
        }
        SessionOutputFormat::Table => {
            if sessions.is_empty() {
                println!("No saved sessions.");
                return Ok(());
            }
            println!(
                "SESSION                           PROVIDER  TURN  PHASE         PROJECT / TASK"
            );
            for state in sessions {
                let remote_project = state
                    .chatgpt_project
                    .as_ref()
                    .and_then(|project| project.name.as_deref())
                    .map(|name| format!(" [ChatGPT:{name}]"))
                    .unwrap_or_default();
                println!(
                    "{:<32}  {:<8}  {:>4}  {:<12}  {}{} / {}",
                    state.session_id,
                    format!("{:?}", state.provider).to_ascii_lowercase(),
                    state.turn,
                    state.phase,
                    state.project_root.display(),
                    remote_project,
                    state.task.lines().next().unwrap_or_default()
                );
            }
        }
    }
    Ok(())
}

fn print_session(state: &SessionState, format: SessionOutputFormat) -> Result<()> {
    match format {
        SessionOutputFormat::Json => println!("{}", serde_json::to_string_pretty(state)?),
        SessionOutputFormat::Table => {
            println!("session: {}", state.session_id);
            println!(
                "provider: {}",
                format!("{:?}", state.provider).to_ascii_lowercase()
            );
            println!("project: {}", state.project_root.display());
            if let Some(project) = state.chatgpt_project.as_ref() {
                println!(
                    "chatgpt_project: {}",
                    project.name.as_deref().unwrap_or("<unnamed>")
                );
                println!("chatgpt_project_id: {}", project.project_id);
                println!("chatgpt_project_url: {}", project.url);
            }
            println!("phase: {}", state.phase);
            println!("turn: {}", state.turn);
            println!(
                "mode: {}",
                state.active_mode.as_deref().unwrap_or("site-current")
            );
            println!(
                "conversation: {}",
                state.conversation_url.as_deref().unwrap_or("-")
            );
            println!("created_at_ms: {}", state.created_at_ms);
            println!("updated_at_ms: {}", state.updated_at_ms);
            println!("task: {}", state.task);
            if let Some(message) = state.last_message.as_deref() {
                println!("last_message: {message}");
            }
        }
    }
    Ok(())
}

async fn run_new(cli: &Cli, task: String, files: Vec<PathBuf>) -> Result<()> {
    let provider = cli.model.unwrap_or_default();
    let config = configured(cli, provider, cli.project.clone())?;
    validate_files(&files)?;

    let project_binding = match cli.chatgpt_project.as_deref() {
        Some(target) if provider == ProviderId::Chatgpt => {
            Some(resolve_chatgpt_project(&config, target).await?)
        }
        Some(_) => {
            return Err(WtError::Config(
                "--chatgpt-project can only be used with --model chatgpt".into(),
            ))
        }
        None => None,
    };

    let mut session =
        SessionStore::create(&config.app_data_dir, provider, &config.project_root, task).await?;
    if let Some(binding) = project_binding {
        eprintln!(
            "chatgpt project: {} ({})",
            binding.name.as_deref().unwrap_or(&binding.project_id),
            binding.url
        );
        session.state.conversation_url = Some(binding.url.clone());
        session.state.chatgpt_project = Some(binding.clone());
        session.save().await?;
        session
            .append_event(
                "chatgpt.project_bound",
                serde_json::json!({"project": binding}),
            )
            .await?;
    }

    let session_id = session.state.session_id.clone();
    eprintln!("session: {session_id}");
    let runtime = runtime_from(config, session);
    let result = runtime.run(false, None, files, cli.mode.clone()).await?;
    println!("{result}");
    Ok(())
}

async fn resume(cli: &Cli, session_id: &str, instruction: String) -> Result<()> {
    if cli.chatgpt_project.is_some() {
        return Err(WtError::Config(
            "--chatgpt-project is only used when creating a new session; resume uses the Project binding saved in the session"
                .into(),
        ));
    }
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
    println!("  version: {}", env!("CARGO_PKG_VERSION"));
    println!("  binary: {}", current_executable_display());
    println!("  project: {}", config.project_root.display());
    println!(
        "  provider: {} ({})",
        provider.label(),
        provider.config().base_url
    );
    if let Some(target) = cli.chatgpt_project.as_deref() {
        println!("  chatgpt project target: {target}");
    }
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

fn current_executable_display() -> String {
    std::env::current_exe()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|error| format!("<unavailable: {error}>"))
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

#[cfg(test)]
mod cli_tests {
    use super::*;

    #[test]
    fn session_without_subcommand_is_management_not_task() {
        let cli = Cli::try_parse_from(["wtagent", "session"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Session { command: None })
        ));
    }

    #[test]
    fn parses_session_list_json() {
        let cli = Cli::try_parse_from([
            "wtagent", "session", "list", "--limit", "5", "--format", "json",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Session {
                command: Some(SessionCommands::List {
                    limit: 5,
                    format: SessionOutputFormat::Json,
                })
            })
        ));
    }

    #[test]
    fn parses_chatgpt_project_target() {
        let cli = Cli::try_parse_from([
            "wtagent",
            "run",
            "--chatgpt-project",
            "OpenSource",
            "inspect",
        ])
        .unwrap();
        assert_eq!(cli.chatgpt_project.as_deref(), Some("OpenSource"));
        assert!(matches!(cli.command, Some(Commands::Run { .. })));
    }

    #[test]
    fn parses_chatgpt_projects_command() {
        let cli =
            Cli::try_parse_from(["wtagent", "chatgpt", "projects", "--format", "json"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Chatgpt {
                command: ChatGptCommands::Projects {
                    format: SessionOutputFormat::Json
                }
            })
        ));
    }
}
