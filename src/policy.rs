use std::path::{Component, Path, PathBuf};

use crate::{
    config::ApprovalMode,
    error::{Result, WtError},
    tools::ToolRisk,
};

#[derive(Debug, Clone)]
pub struct PolicyEngine {
    project_root: PathBuf,
    approval: ApprovalMode,
}

impl PolicyEngine {
    pub fn new(project_root: PathBuf, approval: ApprovalMode) -> Self {
        Self {
            project_root,
            approval,
        }
    }

    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    pub fn approval_mode(&self) -> ApprovalMode {
        self.approval
    }

    pub fn requires_confirmation(&self, risk: ToolRisk) -> Result<bool> {
        match (self.approval, risk) {
            (_, ToolRisk::Read) => Ok(false),
            (ApprovalMode::Auto, ToolRisk::Write | ToolRisk::Execute) => Ok(false),
            (ApprovalMode::Ask, ToolRisk::Write | ToolRisk::Execute) => Ok(true),
            (ApprovalMode::ReadOnly, _) => Err(WtError::Policy(
                "read-only approval mode denies write/execute tools".into(),
            )),
        }
    }

    pub fn resolve_read_path(&self, raw: &str) -> Result<PathBuf> {
        let candidate = self.lexical_project_path(raw)?;
        let canonical = candidate.canonicalize().map_err(|e| {
            WtError::Policy(format!("cannot resolve path {}: {e}", candidate.display()))
        })?;
        self.ensure_inside(&canonical)?;
        Ok(canonical)
    }

    pub fn resolve_write_path(&self, raw: &str) -> Result<PathBuf> {
        let candidate = self.lexical_project_path(raw)?;
        if candidate.exists() {
            let canonical = candidate.canonicalize().map_err(|e| {
                WtError::Policy(format!("cannot resolve path {}: {e}", candidate.display()))
            })?;
            self.ensure_inside(&canonical)?;
            return Ok(canonical);
        }

        // Walk upward until an existing ancestor is found. Canonicalizing it
        // catches a symlink that would otherwise redirect a new file outside
        // the project root.
        let mut ancestor = candidate.parent();
        while let Some(path) = ancestor {
            if path.exists() {
                let canonical = path.canonicalize().map_err(|e| {
                    WtError::Policy(format!("cannot resolve path {}: {e}", path.display()))
                })?;
                self.ensure_inside(&canonical)?;
                return Ok(candidate);
            }
            ancestor = path.parent();
        }
        Err(WtError::Policy(format!(
            "no existing ancestor for write target {}",
            candidate.display()
        )))
    }

    fn lexical_project_path(&self, raw: &str) -> Result<PathBuf> {
        let path = Path::new(raw);
        if path.is_absolute() {
            return Err(WtError::Policy(format!(
                "absolute paths are outside the project policy: {raw}"
            )));
        }
        if path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
        {
            return Err(WtError::Policy(format!(
                "parent traversal is not allowed: {raw}"
            )));
        }
        Ok(self.project_root.join(path))
    }

    fn ensure_inside(&self, path: &Path) -> Result<()> {
        if !path.starts_with(&self.project_root) {
            return Err(WtError::Policy(format!(
                "path is outside project root: {}",
                path.display()
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_parent_traversal() {
        let temp = tempfile::tempdir().unwrap();
        let policy = PolicyEngine::new(temp.path().canonicalize().unwrap(), ApprovalMode::Ask);
        assert!(policy.resolve_write_path("../escape.txt").is_err());
    }
}
