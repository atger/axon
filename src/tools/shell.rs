use std::process::Command;

use serde_json::Value;

use super::{Tool, ToolError};

pub struct RunCommandTool;

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

    fn execute(&self, args: Value) -> Result<String, ToolError> {
        let cmd = args["cmd"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("missing 'cmd'".into()))?;

        let output = Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .output()
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
