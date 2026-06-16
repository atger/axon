pub mod fs;
pub mod shell;
pub mod web;

use serde_json::Value;

#[derive(thiserror::Error, Debug)]
pub enum ToolError {
    #[error("unknown tool: {0}")]
    UnknownTool(String),
    #[error("invalid args: {0}")]
    InvalidArgs(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("command failed: {0}")]
    CommandFailed(String),
}

pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn is_destructive(&self) -> bool {
        false
    }
    fn execute(&self, args: Value) -> Result<String, ToolError>;
}

pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }

    pub fn with_defaults() -> Self {
        Self::new()
            .register(Box::new(fs::ReadFileTool))
            .register(Box::new(fs::WriteFileTool))
            .register(Box::new(shell::RunCommandTool))
            .register(Box::new(web::WebSearchTool))
    }

    pub fn register(mut self, tool: Box<dyn Tool>) -> Self {
        self.tools.push(tool);
        self
    }

    pub fn is_destructive(&self, name: &str) -> bool {
        self.tools
            .iter()
            .find(|t| t.name() == name)
            .is_some_and(|t| t.is_destructive())
    }

    pub fn execute(&self, name: &str, args: Value) -> Result<String, ToolError> {
        match self.tools.iter().find(|t| t.name() == name) {
            Some(tool) => tool.execute(args),
            None => Err(ToolError::UnknownTool(name.to_string())),
        }
    }

    /// Returns a minimal tool-instruction block for the system prompt.
    pub fn system_prompt_section(&self) -> String {
        let mut s = String::from(
            "## Tools\n\
             /no_think\n\
             Always output exactly one JSON object — no surrounding text.\n\
             To answer: {\"type\":\"text\",\"content\":\"your answer\"}\n\
             To use a tool: {\"type\":\"tool_call\",\"name\":\"tool\",\"args\":{...}}\n\n\
             Available tools:\n",
        );
        for tool in &self.tools {
            s.push_str(&format!("- {}: {}\n", tool.name(), tool.description()));
        }
        s
    }
}
