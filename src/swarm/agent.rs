//! Agents for the swarm path, wrapped in a ReAct executor.
//!
//! `AgentDeriveT` is hand-implemented (not the `#[agent]` macro) so each agent
//! can carry a **unique name** (the actor runtime registers actors by name) and
//! a **role** that selects its scoped toolset + system prompt. Safety for the
//! built-in pipeline is structural: a role only gets the tools it needs.
//! Per-agent cancellation and (for generic `Coder` agents) a tool approval
//! policy are enforced via `AgentHooks`.

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

use crate::swarm::tools::{AddTaskTool, RunCommandTool, WebSearchTool};

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
    /// Researches other agentic apps for frontend ideas (web + read-only).
    Researcher,
    /// Plans concrete tasks from research findings (add_task only).
    Planner,
    /// Implements an accepted OKF by editing frontend files (no shell).
    Developer,
}

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

const CODER_PROMPT: &str = "You are axon, a local AI coding agent using the ReAct (Reasoning + Acting) pattern. \
Solve software tasks by alternating Thought, Action (tool call), and Observation until done. \
Tools: read_file, write_file, list_dir, search_file, delete_file (filesystem); \
run_command (shell, via sh -c); web_search. Be concise; make incremental changes; use exact paths; \
never delete or overwrite without clear intent. When done, summarize what you did.";

const RESEARCHER_PROMPT: &str = "You are axon's frontend researcher. Study how OTHER agentic / AI-agent \
applications design their dashboards, UX, and features (use web_search). Use list_dir/read_file under \
`frontend/` to understand axon's current dashboard. For EACH concrete, high-value idea worth adopting, \
call the `add_task` tool exactly once (title, one-sentence description, comma-separated tags, and a \
markdown body containing Rationale, Affected files under `frontend/`, and the proposed change). Do not \
edit frontend files — only `add_task`. Avoid duplicating ideas already in the queue.";

const PLANNER_PROMPT: &str = "You are axon's planner. You are given an APPROVED frontend task. Read the \
relevant files under `frontend/` (read_file, list_dir, search_file) and produce a concrete, \
step-by-step implementation plan: exactly which files to change and what to change (markup, CSS, \
Leptos code). Output the plan as markdown. Planning only — do NOT edit files.";

const DEVELOPER_PROMPT: &str = "You are axon's frontend developer. Implement the given accepted OKF \
suggestion by editing files under `frontend/` (read_file, list_dir, search_file, write_file). Make \
minimal, correct changes consistent with the existing Leptos (CSR) code so the project still compiles. \
Do NOT run shell commands — building is handled for you. If told a previous build failed, fix those \
errors and continue. When finished, summarize the files you changed.";

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
}

impl CodingAgent {
    pub fn with_role(name: impl Into<String>, role: Role, control: Arc<AgentControl>) -> Self {
        Self {
            name: name.into(),
            role,
            control,
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
        match self.role {
            Role::Coder => CODER_PROMPT,
            Role::Researcher => RESEARCHER_PROMPT,
            Role::Planner => PLANNER_PROMPT,
            Role::Developer => DEVELOPER_PROMPT,
        }
    }

    fn output_schema(&self) -> Option<Value> {
        None
    }

    fn tools(&self) -> Vec<Box<dyn ToolT>> {
        match self.role {
            Role::Coder => vec![
                Box::new(ReadFile::new()),
                Box::new(WriteFile::new()),
                Box::new(ListDir::new()),
                Box::new(SearchFile::new(100)),
                Box::new(DeleteFile::new()),
                Box::new(RunCommandTool {}),
                Box::new(WebSearchTool {}),
            ],
            Role::Researcher => vec![
                Box::new(WebSearchTool {}),
                Box::new(ReadFile::new()),
                Box::new(ListDir::new()),
                Box::new(SearchFile::new(100)),
                Box::new(AddTaskTool {}),
            ],
            Role::Planner => vec![
                Box::new(ReadFile::new()),
                Box::new(ListDir::new()),
                Box::new(SearchFile::new(100)),
            ],
            Role::Developer => vec![
                Box::new(ReadFile::new()),
                Box::new(WriteFile::new()),
                Box::new(ListDir::new()),
                Box::new(SearchFile::new(100)),
            ],
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
