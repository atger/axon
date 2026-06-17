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
    /// Whether this specific invocation requires user confirmation.
    /// Defaults to `is_destructive()`; tools with arg-dependent safety can override.
    fn needs_confirm(&self, _args: &Value) -> bool {
        self.is_destructive()
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

    pub fn needs_confirm(&self, name: &str, args: &Value) -> bool {
        self.tools
            .iter()
            .find(|t| t.name() == name)
            .is_some_and(|t| t.needs_confirm(args))
    }

    pub fn execute(&self, name: &str, args: Value) -> Result<String, ToolError> {
        match self.tools.iter().find(|t| t.name() == name) {
            Some(tool) => tool.execute(args),
            None => Err(ToolError::UnknownTool(name.to_string())),
        }
    }

    /// Returns the complete system prompt: persona, tool instructions, and tool list.
    pub fn system_prompt_section(&self) -> String {
        let today = chrono::Local::now().format("%Y-%m-%d");
        let mut s = format!(
            "You are Axon, a concise local AI coding assistant. Today is {today}.\n\n\
             ## Tools\n\
             /no_think\n\
             Always output exactly one JSON object — no surrounding text. \
             Never say you don't know; use web_search instead.\n\
             To answer: {{\"type\":\"text\",\"content\":\"your answer\"}}\n\
             To use a tool: {{\"type\":\"tool_call\",\"name\":\"tool\",\"args\":{{...}}}}\n\n\
             Available tools:\n",
        );
        for tool in &self.tools {
            s.push_str(&format!("- {}: {}\n", tool.name(), tool.description()));
        }
        s
    }
}
