//! Custom AutoAgents tools for the swarm path: a shell runner and a web search
//! tool. Filesystem tools are reused from `autoagents-toolkit`. The logic mirrors
//! the legacy `crate::tools::{shell, web}` implementations.

use autoagents::async_trait;
use autoagents::core::tool::{ToolCallError, ToolRuntime, ToolT, ToolInputT};
use autoagents_derive::{ToolInput, tool};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::process::Command;
use tokio::sync::mpsc::UnboundedSender;
use std::sync::Arc;

use crate::swarm::SpawnCmd;
use crate::swarm::store;
use crate::tools::mcp::McpTool;

#[derive(Debug)]
pub struct McpSwarmTool {
    inner: McpTool,
}

impl McpSwarmTool {
    pub fn new(inner: McpTool) -> Self {
        Self { inner }
    }
}

impl ToolT for McpSwarmTool {
    fn name(&self) -> &str {
        crate::tools::Tool::name(&self.inner)
    }

    fn description(&self) -> &str {
        crate::tools::Tool::description(&self.inner)
    }

    fn args_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": true
        })
    }
}

#[async_trait]
impl ToolRuntime for McpSwarmTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        crate::tools::Tool::execute(&self.inner, args).await
            .map(|s| Value::String(s))
            .map_err(|e| ToolCallError::RuntimeError(e.to_string().into()))
    }
}

#[derive(Debug)]
pub struct SharedTool {
    inner: Arc<dyn ToolT>,
}

impl SharedTool {
    pub fn new(inner: Arc<dyn ToolT>) -> Self {
        Self { inner }
    }
}

impl ToolT for SharedTool {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn description(&self) -> &str {
        self.inner.description()
    }
    fn args_schema(&self) -> Value {
        self.inner.args_schema()
    }
}

#[async_trait]
impl ToolRuntime for SharedTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        self.inner.execute(args).await
    }
}

#[derive(Serialize, Deserialize, ToolInput, Debug)]
pub struct AddTaskArgs {
    #[input(description = "Short, specific title for the task")]
    title: String,
    #[input(description = "One-sentence summary of the task")]
    description: String,
    #[input(description = "Comma-separated tags, e.g. 'ux, leptos'")]
    tags: String,
    #[input(
        description = "Markdown body: rationale, affected files (under frontend/), and the concrete change"
    )]
    body: String,
}

/// Adds one frontend task to the queue. Validation happens here, so the planner
/// agent cannot produce a malformed task record.
#[tool(
    name = "add_task",
    description = "Add one concrete frontend task to the tasks queue (status: proposed). Returns its id.",
    input = AddTaskArgs
)]
pub struct AddTaskTool {}

#[async_trait]
impl ToolRuntime for AddTaskTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let args: AddTaskArgs = serde_json::from_value(args)?;
        if args.title.trim().is_empty() {
            return Err(ToolCallError::RuntimeError("title must not be empty".into()));
        }
        let id = store::add_task(&args.title, &args.description, &args.tags, &args.body)
            .map_err(|e| ToolCallError::RuntimeError(e.to_string().into()))?;
        Ok(format!("added task `{id}`").into())
    }
}

#[derive(Serialize, Deserialize, ToolInput, Debug)]
pub struct SpawnAgentArgs {
    #[input(
        description = "Definition id of the agent to spawn (e.g. 'agent-writer' or a user agent's id)"
    )]
    def_id: String,
    #[input(description = "The task for the spawned agent to work on")]
    task: String,
}

/// Lets a (typically proactive) agent delegate by spawning another configured
/// agent. The spawn is performed by the swarm via a channel, so the tool holds
/// no reference back to the swarm (no `Arc` cycle).
#[tool(
    name = "spawn_agent",
    description = "Spawn another configured agent (by its definition id) to work on a task in parallel.",
    input = SpawnAgentArgs
)]
pub struct SpawnAgentTool {
    pub tx: UnboundedSender<SpawnCmd>,
}

#[async_trait]
impl ToolRuntime for SpawnAgentTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let args: SpawnAgentArgs = serde_json::from_value(args)?;
        if args.task.trim().is_empty() {
            return Err(ToolCallError::RuntimeError("task must not be empty".into()));
        }
        self.tx
            .send(SpawnCmd {
                def_id: args.def_id.clone(),
                task: args.task,
            })
            .map_err(|e| ToolCallError::RuntimeError(format!("failed to queue spawn: {e}").into()))?;
        Ok(format!("requested spawn of agent `{}`", args.def_id).into())
    }
}

#[derive(Serialize, Deserialize, ToolInput, Debug)]
pub struct RunCommandArgs {
    #[input(description = "The shell command to run")]
    cmd: String,
}

#[tool(
    name = "run_command",
    description = "Run a shell command via `sh -c` and return its combined stdout/stderr.",
    input = RunCommandArgs
)]
pub struct RunCommandTool {}

#[async_trait]
impl ToolRuntime for RunCommandTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let args: RunCommandArgs = serde_json::from_value(args)?;
        let output = Command::new("sh")
            .arg("-c")
            .arg(&args.cmd)
            .output()
            .map_err(|e| ToolCallError::RuntimeError(e.to_string().into()))?;

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
                return Err(ToolCallError::RuntimeError(
                    format!("command failed: exit code {code}").into(),
                ));
            }
            result.push_str(&format!("\n[exit code {code}]"));
        }
        Ok(result.into())
    }
}

#[derive(Serialize, Deserialize, ToolInput, Debug)]
pub struct WebSearchArgs {
    #[input(description = "The search query")]
    query: String,
}

#[tool(
    name = "web_search",
    description = "Search the web; use for current events, news, or anything you are unsure about.",
    input = WebSearchArgs
)]
pub struct WebSearchTool {}

#[async_trait]
impl ToolRuntime for WebSearchTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let args: WebSearchArgs = serde_json::from_value(args)?;
        let encoded = args.query.replace(' ', "+");
        let url = format!("https://html.duckduckgo.com/html/?q={encoded}");

        // Blocking HTTP via ureq, off the async runtime's worker concerns is
        // acceptable for v1 (mirrors the legacy web tool).
        let html = ureq::get(&url)
            .set("User-Agent", "Mozilla/5.0 (compatible; axon/0.1)")
            .call()
            .map_err(|e| ToolCallError::RuntimeError(format!("search request failed: {e}").into()))?
            .into_string()
            .map_err(|e| ToolCallError::RuntimeError(format!("bad search response: {e}").into()))?;

        let snippets = extract_snippets(&html, 5);
        if snippets.is_empty() {
            return Ok(format!("No results found for: {}", args.query).into());
        }
        Ok(snippets.join("\n").into())
    }
}

/// Extract up to `limit` result snippets from DuckDuckGo HTML results.
fn extract_snippets(html: &str, limit: usize) -> Vec<String> {
    let mut snippets = Vec::new();
    let marker = "class=\"result__snippet\"";
    let mut remaining = html;
    while snippets.len() < limit {
        let Some(pos) = remaining.find(marker) else {
            break;
        };
        remaining = &remaining[pos + marker.len()..];
        let Some(gt) = remaining.find('>') else {
            break;
        };
        remaining = &remaining[gt + 1..];
        let Some(end) = remaining.find("</a>") else {
            break;
        };
        let text = html_clean(remaining[..end].trim());
        if !text.is_empty() {
            snippets.push(text);
        }
    }
    snippets
}

/// Strip HTML tags and unescape common entities.
fn html_clean(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&nbsp;", " ")
}
