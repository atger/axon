use serde_json::Value;

use super::{Tool, ToolError};

pub struct ReadFileTool;
pub struct ListDirTool;
pub struct WriteFileTool;

impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "read_file(path: string) — read the contents of a file"
    }

    fn execute(&self, args: Value) -> Result<String, ToolError> {
        let path = args["path"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("missing 'path'".into()))?;
        let contents = std::fs::read_to_string(path)?;
        // Truncate to ~8000 chars to stay within small model context budgets.
        const LIMIT: usize = 8000;
        if contents.len() > LIMIT {
            Ok(format!(
                "{}\n[truncated — file is {} bytes, showing first {LIMIT}]",
                &contents[..LIMIT],
                contents.len()
            ))
        } else {
            Ok(contents)
        }
    }
}

impl Tool for ListDirTool {
    fn name(&self) -> &str {
        "list_dir"
    }

    fn description(&self) -> &str {
        "list_dir(path: string) — list files and directories at the given path"
    }

    fn execute(&self, args: Value) -> Result<String, ToolError> {
        let path = args["path"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("missing 'path'".into()))?;
        let mut entries: Vec<String> = std::fs::read_dir(path)?
            .filter_map(|e| e.ok())
            .map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                if e.file_type().is_ok_and(|ft| ft.is_dir()) {
                    format!("{name}/")
                } else {
                    name
                }
            })
            .collect();
        entries.sort();
        Ok(entries.join("\n"))
    }
}

impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "write_file(path: string, content: string) — write content to a file (creates or overwrites)"
    }

    fn is_destructive(&self) -> bool {
        true
    }

    fn execute(&self, args: Value) -> Result<String, ToolError> {
        let path = args["path"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("missing 'path'".into()))?;
        let content = args["content"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("missing 'content'".into()))?;
        std::fs::write(path, content)?;
        Ok(format!("wrote {} bytes to {path}", content.len()))
    }
}
