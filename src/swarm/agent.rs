//! Agents for the swarm path, wrapped in a ReAct executor.
//!
//! `AgentDeriveT` is hand-implemented (not the `#[agent]` macro) so each agent
//! can carry a **unique name** (the actor runtime registers actors by name) and
//! a **role** that selects its scoped toolset + system prompt.
//! Per-agent cancellation and tool approval policy are enforced via `AgentHooks`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use autoagents::async_trait;
use autoagents::core::agent::task::Task;
use autoagents::core::agent::{AgentDeriveT, AgentHooks, Context, HookOutcome};
use autoagents::core::tool::ToolT;
use autoagents::llm::ToolCall;
use autoagents_toolkit::tools::filesystem::{DeleteFile, ListDir, ReadFile, SearchFile, WriteFile};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

use crate::swarm::SpawnCmd;
use crate::swarm::tools::{AddTaskTool, RunCommandTool, SpawnAgentTool, WebSearchTool, SharedTool};

/// Approval policy for generic, user-spawned `Coder` agents (the built-in
/// pipeline roles are constrained by their toolset instead, and run AutoApprove).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalPolicy {
    #[default]
    AutoApprove,
    /// Refuse destructive tools (shell + file writes/deletes); allow read-only.
    DenyDestructive,
}

/// The job an agent performs; selects its prompt and tools.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Generic user-spawned coding agent (full toolset, governed by policy).
    Coder,
}

const DESTRUCTIVE_TOOLS: &[&str] = &[
    "run_command",
    "write_file",
    "delete_file",
    "move_file",
    "copy_file",
    "create_dir",
];

/// Read-only filesystem tools, always granted to a `Coder` regardless of the
/// configured toolset (an agent that cannot read is useless).
pub const READONLY_TOOLS: &[&str] = &["read_file", "list_dir", "search_file"];

/// Every tool a generic `Coder` agent can be granted (the full default set).
pub const ALL_CODER_TOOLS: &[&str] = &[
    "read_file",
    "write_file",
    "list_dir",
    "search_file",
    "delete_file",
    "run_command",
    "web_search",
    "add_task",
];

pub fn is_destructive(tool_name: &str) -> bool {
    DESTRUCTIVE_TOOLS.contains(&tool_name)
}

pub const AGENT_WRITER_PROMPT: &str = "\
You are the Agent Writer. Given a description, output ONLY an agent definition with YAML frontmatter + markdown body.\
 No commentary, no questions.

Available tools: write_file, read_file, list_dir, search_file, delete_file, run_command, web_search, add_task.

Format:
---
name: <name>
model:  # leave empty to use default
tools:
  - write_file
  - add_task
  # - run_command
  # - web_search
policy: auto_approve  # or deny_destructive
memory_window: 20
max_turns: 10
schedule_mins: 15  # runs every 15 min
task:  # optional, recurring task
task_hint:  # optional
---

# Agent instructions

<what this agent does>

When done, call add_task(title=<name>, body=<markdown>) to submit your work for human review (body must be markdown).

Generate the definition, then call add_task(title=<name>, body=<full definition>) to submit it for review. \
Output only the definition body — no preamble.";

pub const CODER_PROMPT: &str = "You are axon, a local AI coding agent using the ReAct (Reasoning + Acting) pattern. \
Solve software tasks by alternating Thought, Action (tool call), and Observation until done. \
Tools: read_file, write_file, list_dir, search_file, delete_file (filesystem); \
run_command (shell, via sh -c); web_search. Be concise; make incremental changes; use exact paths; \
never delete or overwrite without clear intent. When done, summarize what you did.";

/// General-purpose research agent (distinct from the pipeline's frontend-specific
/// `RESEARCHER_PROMPT`): gathers information from the web and the local codebase.
pub const RESEARCH_AGENT_PROMPT: &str = "You are a research agent using the ReAct pattern. Investigate the \
user's question thoroughly: use web_search for external information and read_file/list_dir/search_file to \
study the local codebase. Cross-check sources, be concise, and finish with a clear, well-organized summary \
of your findings (with sources where relevant). Do not modify any files.";

/// Read-only code-review agent.
pub const REVIEWER_PROMPT: &str = "You are a code-review agent using the ReAct pattern. Read the relevant \
code with read_file/list_dir/search_file and review it for correctness bugs, security risks, and concrete \
improvements. Cite findings as `file:line` and explain the impact and a suggested fix for each. Be \
specific and prioritize high-confidence issues. You are read-only — do NOT edit, write, or run anything.";



/// Shared, runtime-mutable control surface for a single agent.
pub struct AgentControl {
    cancelled: AtomicBool,
    policy: ApprovalPolicy,
}

impl AgentControl {
    pub fn new(policy: ApprovalPolicy) -> Arc<Self> {
        Arc::new(Self {
            cancelled: AtomicBool::new(false),
            policy,
        })
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    fn gate_tool(&self, tool_name: &str) -> HookOutcome {
        match self.policy {
            ApprovalPolicy::AutoApprove => HookOutcome::Continue,
            ApprovalPolicy::DenyDestructive if is_destructive(tool_name) => HookOutcome::Abort,
            ApprovalPolicy::DenyDestructive => HookOutcome::Continue,
        }
    }
}

/// A ReAct agent. `name` must be unique per spawn (used as the actor name).
#[derive(Clone)]
pub struct CodingAgent {
    name: String,
    role: Role,
    control: Arc<AgentControl>,
    /// System prompt. For system roles this is the role's constant; for a
    /// configured `Coder` it is the agent definition's instructions.
    prompt: String,
    /// When `Some`, restrict a `Coder`'s toolset to these tool names (read-only
    /// tools are always retained). `None` ⇒ the full default `Coder` toolset.
    allowed_tools: Option<Vec<String>>,
    /// Channel for the `spawn_agent` tool (proactive agents delegate through it).
    /// `None` ⇒ the agent cannot spawn others.
    spawn_tx: Option<UnboundedSender<SpawnCmd>>,
    mcp_tools: Vec<Arc<dyn ToolT>>,
}

impl CodingAgent {
    /// A built-in agent: prompt + tools fixed by role.
    pub fn with_role(
        name: impl Into<String>,
        role: Role,
        control: Arc<AgentControl>,
        mcp_tools: Vec<Arc<dyn ToolT>>,
    ) -> Self {
        let prompt = CODER_PROMPT.to_string();
        Self {
            name: name.into(),
            role,
            control,
            prompt,
            allowed_tools: None,
            spawn_tx: None,
            mcp_tools,
        }
    }

    /// A configured generic `Coder` agent: custom system prompt, an optional
    /// restricted toolset, and an optional channel for spawning other agents.
    pub fn coder(
        name: impl Into<String>,
        control: Arc<AgentControl>,
        prompt: String,
        allowed_tools: Option<Vec<String>>,
        spawn_tx: Option<UnboundedSender<SpawnCmd>>,
        mcp_tools: Vec<Arc<dyn ToolT>>,
    ) -> Self {
        Self {
            name: name.into(),
            role: Role::Coder,
            control,
            prompt,
            allowed_tools,
            spawn_tx,
            mcp_tools,
        }
    }
}

impl std::fmt::Debug for CodingAgent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CodingAgent({}, {:?})", self.name, self.role)
    }
}

impl AgentDeriveT for CodingAgent {
    type Output = String;

    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.prompt
    }

    fn output_schema(&self) -> Option<Value> {
        None
    }

    fn tools(&self) -> Vec<Box<dyn ToolT>> {
        let mut all: Vec<Box<dyn ToolT>> = vec![
            Box::new(ReadFile::new()),
            Box::new(WriteFile::new()),
            Box::new(ListDir::new()),
            Box::new(SearchFile::new(100)),
            Box::new(DeleteFile::new()),
            Box::new(RunCommandTool {}),
            Box::new(WebSearchTool {}),
            Box::new(AddTaskTool {}),
        ];
        if let Some(tx) = &self.spawn_tx {
            all.push(Box::new(SpawnAgentTool { tx: tx.clone() }));
        }

        // Add MCP tools
        for mcp in &self.mcp_tools {
            // We need to clone the dynamic tool.
            // Since we can't easily clone dyn ToolT, we rely on the fact that
            // our McpSwarmTool could be designed to be shared or cloned.
            // Actually, we can just wrap it in a pointer.
            // But ToolT implementation requires it to be a trait object.
            // I'll make a wrapper that holds Arc<dyn ToolT>.
            all.push(Box::new(SharedTool::new(mcp.clone())));
        }

        match &self.allowed_tools {
            // Keep read-only tools always; otherwise honor the allow-list.
            Some(allowed) => all
                .into_iter()
                .filter(|t| {
                    READONLY_TOOLS.contains(&t.name()) || allowed.iter().any(|a| a == t.name())
                })
                .collect(),
            None => all,
        }
    }
}

#[async_trait]
impl AgentHooks for CodingAgent {
    async fn on_run_start(&self, _task: &Task, _ctx: &Context) -> HookOutcome {
        if self.control.is_cancelled() {
            HookOutcome::Abort
        } else {
            HookOutcome::Continue
        }
    }

    async fn on_tool_call(&self, tool_call: &ToolCall, _ctx: &Context) -> HookOutcome {
        if self.control.is_cancelled() {
            return HookOutcome::Abort;
        }
        self.control.gate_tool(&tool_call.function.name)
    }
}
