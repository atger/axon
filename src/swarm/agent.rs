//! The axon coding agent for the swarm path, wrapped in a ReAct executor.
//!
//! `AgentDeriveT` is hand-implemented (rather than via the `#[agent]` macro) so
//! each agent can carry a **unique name** — the actor runtime registers actors
//! by name, so identical names would collide on the second spawn. Per-agent
//! **cancellation** and **tool approval policy** are implemented through
//! `AgentHooks`, which fire at run start and before every tool call.

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

use crate::swarm::tools::{RunCommandTool, WebSearchTool};

/// What an agent is allowed to do with tools without a human in the loop.
///
/// v1 is policy-based and fully local (no interactive round-trip). Interactive
/// per-tool confirmation via the dashboard is a planned follow-up that would add
/// an `Ask` variant backed by a channel awaited inside [`AgentControl`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalPolicy {
    /// Run every tool without prompting — an autonomous swarm member.
    #[default]
    AutoApprove,
    /// Refuse destructive tools (shell + file writes/deletes); allow read-only.
    DenyDestructive,
}

/// Tools that mutate the filesystem or run arbitrary shell commands.
const DESTRUCTIVE_TOOLS: &[&str] = &[
    "run_command",
    "write_file",
    "delete_file",
    "move_file",
    "copy_file",
    "create_dir",
];

pub fn is_destructive(tool_name: &str) -> bool {
    DESTRUCTIVE_TOOLS.contains(&tool_name)
}

const SYSTEM_PROMPT: &str = "You are axon, a local AI coding agent using the ReAct (Reasoning + Acting) pattern. \
Solve software tasks by alternating Thought, Action (tool call), and Observation until done. \
Tools: read_file, write_file, list_dir, search_file, delete_file (filesystem); \
run_command (shell, via sh -c); web_search (current info you lack). \
Principles: prefer reading and listing before editing; use exact paths; make incremental changes; \
follow existing code style; never delete or overwrite without clear intent; be concise. \
When the task is complete, respond with a short summary of what you did.";

/// Shared, runtime-mutable control surface for a single agent. Held by the
/// registry (to cancel) and by the agent's hooks (to enforce cancel + policy).
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

    /// Request cooperative cancellation. Takes effect at the next hook boundary
    /// (run start or tool call).
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

/// The coding agent. `name` must be unique per spawn (used as the actor name).
#[derive(Clone)]
pub struct CodingAgent {
    name: String,
    control: Arc<AgentControl>,
}

impl CodingAgent {
    pub fn new(name: impl Into<String>, control: Arc<AgentControl>) -> Self {
        Self {
            name: name.into(),
            control,
        }
    }
}

impl std::fmt::Debug for CodingAgent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CodingAgent({})", self.name)
    }
}

impl AgentDeriveT for CodingAgent {
    type Output = String;

    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        SYSTEM_PROMPT
    }

    fn output_schema(&self) -> Option<Value> {
        None
    }

    fn tools(&self) -> Vec<Box<dyn ToolT>> {
        vec![
            Box::new(ReadFile::new()),
            Box::new(WriteFile::new()),
            Box::new(ListDir::new()),
            Box::new(SearchFile::new(100)),
            Box::new(DeleteFile::new()),
            Box::new(RunCommandTool {}),
            Box::new(WebSearchTool {}),
        ]
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
