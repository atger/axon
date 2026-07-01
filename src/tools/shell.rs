use async_trait::async_trait;
use serde_json::Value;
use tokio::process::Command;

use super::{Tool, ToolError};

/// Safe read-only binaries that never require confirmation.
const SAFE_BINS: &[&str] = &[
    "ls", "ll", "la", "find", "tree", "cat", "head", "tail", "wc", "less", "more", "file", "stat",
    "grep", "rg", "ag", "awk", "sed", "pwd", "whoami", "date", "uname", "hostname", "id", "ps",
    "df", "du", "free", "vmstat", "uptime", "echo", "printf", "which", "type", "lsof", "cargo",
];

/// Safe `git` subcommands (read-only).
const SAFE_GIT_SUBCMDS: &[&str] = &[
    "status",
    "log",
    "diff",
    "branch",
    "show",
    "blame",
    "describe",
    "shortlog",
    "reflog",
    "ls-files",
    "ls-tree",
    "rev-parse",
];

fn is_safe_command(cmd: &str) -> bool {
    let mut tokens = cmd.split_whitespace();
    let bin = match tokens.next() {
        Some(b) => b.rsplit('/').next().unwrap_or(b),
        None => return false,
    };
    if bin == "git" {
        let sub = tokens.next().unwrap_or("");
        return SAFE_GIT_SUBCMDS.contains(&sub);
    }
    SAFE_BINS.contains(&bin)
}

pub struct RunCommandTool;

#[async_trait]
impl Tool for RunCommandTool {
    fn name(&self) -> &str {
        "run_command"
    }

    fn description(&self) -> &str {
        "run_command(cmd: string) — run a shell command and return its output"
    }

    fn is_destructive(&self) -> bool {
        true
    }

    fn needs_confirm(&self, args: &Value) -> bool {
        let cmd = args["cmd"].as_str().unwrap_or("");
        !is_safe_command(cmd)
    }

    async fn execute(&self, args: Value) -> Result<String, ToolError> {
        let cmd = args["cmd"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("missing 'cmd'".into()))?;

        let output = Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .output()
            .await
            .map_err(ToolError::Io)?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        let mut result = String::new();
        if !stdout.is_empty() {
            result.push_str(stdout.trim_end());
        }
        if !stderr.is_empty() {
            if !result.is_empty() {
                result.push('\n');
            }
            result.push_str("[stderr] ");
            result.push_str(stderr.trim_end());
        }
        if !output.status.success() {
            let code = output.status.code().unwrap_or(-1);
            if result.is_empty() {
                return Err(ToolError::CommandFailed(format!("exit code {code}")));
            }
            result.push_str(&format!("\n[exit code {code}]"));
        }
        Ok(result)
    }
}
