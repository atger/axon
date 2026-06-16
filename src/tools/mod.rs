pub mod fs;
pub mod shell;

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
            .register(Box::new(fs::ListDirTool))
            .register(Box::new(fs::WriteFileTool))
            .register(Box::new(shell::RunCommandTool))
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

    /// Generates a GBNF grammar that constrains the model to output exactly one of:
    ///   {"type":"text","content":"<answer>"}
    ///   {"type":"tool_call","name":"<tool>","args":{...}}
    pub fn build_grammar(&self) -> String {
        let name_enum: String = self
            .tools
            .iter()
            .map(|t| format!("\"\\\"{}\\\"\"", t.name()))
            .collect::<Vec<_>>()
            .join(" | ");

        // The format! call uses {{ / }} to produce literal { / } in the output.
        // The GBNF string-char rule uses [^"\\] (any char except " and \) and
        // "\\" (escaped backslash) — these survive both Rust raw-string and format!
        // processing unchanged, and are interpreted correctly by the GBNF parser.
        format!(
            r#"root ::= text-resp | tool-resp
text-resp ::= "{{" ws "\"type\"" ws ":" ws "\"text\"" ws "," ws "\"content\"" ws ":" ws string ws "}}"
tool-resp ::= "{{" ws "\"type\"" ws ":" ws "\"tool_call\"" ws "," ws "\"name\"" ws ":" ws tool-name ws "," ws "\"args\"" ws ":" ws json-object ws "}}"
tool-name ::= {name_enum}
json-object ::= "{{" ws (json-pair ("," ws json-pair)*)? "}}"
json-pair ::= string ws ":" ws json-value
json-value ::= string | json-number | json-object | json-array | "true" | "false" | "null"
json-array ::= "[" ws (json-value ("," ws json-value)*)? "]"
string ::= "\"" string-char* "\""
string-char ::= [^"\\] | "\\" (["\\/bfnrt] | "u" [0-9a-fA-F] [0-9a-fA-F] [0-9a-fA-F] [0-9a-fA-F])
json-number ::= "-"? ([0-9] | [1-9] [0-9]*) ("." [0-9]+)? ([eE] [-+]? [0-9]+)?
ws ::= [ \t\n\r]*
"#,
            name_enum = name_enum
        )
    }

    /// Returns a minimal tool-instruction block for the system prompt.
    pub fn system_prompt_section(&self) -> String {
        let mut s = String::from(
            "## Tools\n\
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
