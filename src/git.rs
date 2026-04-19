use std::fs;
use std::path::PathBuf;
use std::process::Command as ProcessCommand;

use crate::error::{CliError, CliResult};

pub fn require_repo_root(json: bool) -> CliResult<PathBuf> {
    find_repo_root().ok_or_else(|| CliError::Validation {
        message: "current directory is not inside a Git repository".to_string(),
        json,
    })
}

pub fn find_repo_root() -> Option<PathBuf> {
    let output = ProcessCommand::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    fs::canonicalize(stdout.trim()).ok()
}
