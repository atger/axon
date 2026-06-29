//! Swarm orchestration on AutoAgents: a registry of concurrent agents plus the
//! built-in self-improvement pipeline (researcher → okf-writer → human →
//! developer), driven by a shared runtime and a protocol-event pump.

pub mod agent;
pub mod store;
pub mod teams;
pub mod tools;

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use autoagents::core::actor::Topic;
use autoagents::core::agent::memory::SlidingWindowMemory;
use autoagents::core::agent::prebuilt::executor::ReActAgent;
use autoagents::core::agent::task::Task;
use autoagents::core::agent::{ActorAgent, AgentBuilder};
use autoagents::core::environment::Environment;
use autoagents::core::runtime::{SingleThreadedRuntime, TypedRuntime};
use autoagents::llm::LLMProvider;
use autoagents::llm::builder::LLMBuilder;
use autoagents::protocol::{Event, SubmissionId};
use color_eyre::eyre::{Result, WrapErr};
use serde::Serialize;
use tokio::sync::{Mutex, RwLock, broadcast, mpsc};
use tokio_stream::StreamExt;

use agent::{AgentControl, ApprovalPolicy, CodingAgent, Role};
use teams::AgentDef;

const RESEARCHER_ID: &str = "researcher";
const PLANNER_ID: &str = "planner";
const DEVELOPER_ID: &str = "developer";

const RESEARCH_TASK: &str = "Research how other agentic / AI-agent applications design their dashboards, \
UX, and features, and add improvement tasks for axon's web dashboard. First read the current frontend \
under `frontend/` (list_dir + read_file). Then web_search for ideas from other applications. For each \
concrete, high-value idea, call `add_task` once. Avoid duplicating tasks already in the queue.";

/// Lifecycle status of a swarm agent, surfaced to the dashboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Queued,
    Running,
    Done,
    /// A perpetual system agent that finished a cycle and is waiting for the next
    /// trigger (so it reads as "alive", not stopped).
    Idle,
    Error,
    Cancelled,
}

/// A spawned agent as seen by the registry / REST API.
#[derive(Clone, Serialize)]
pub struct AgentInfo {
    pub id: String,
    pub task: String,
    pub model: String,
    pub policy: ApprovalPolicy,
    pub status: AgentStatus,
    pub role: Role,
    /// True for built-in system agents (the pipeline); excluded from cancel-all.
    pub perpetual: bool,
    /// Name of the agent definition this was spawned from (e.g. "Coder"); `None`
    /// for the built-in pipeline agents.
    pub def_name: Option<String>,
}

struct AgentEntry {
    info: AgentInfo,
    control: Arc<AgentControl>,
}

/// An event forwarded to dashboard WebSocket clients.
#[derive(Clone, Serialize)]
pub struct SwarmEvent {
    pub agent_id: String,
    pub event: serde_json::Value,
}

/// Internal construction parameters shared by the system pipeline and configured
/// user agents.
struct AgentSpec {
    id: String,
    role: Role,
    policy: ApprovalPolicy,
    llm: Arc<dyn LLMProvider>,
    /// `None` ⇒ use the role's default prompt (built-in pipeline agents); `Some`
    /// ⇒ a configured `Coder`'s custom instructions.
    prompt: Option<String>,
    allowed_tools: Option<Vec<String>>,
    memory_window: usize,
    max_turns: usize,
    model_label: String,
    perpetual: bool,
    def_name: Option<String>,
    spawn_tx: Option<mpsc::UnboundedSender<SpawnCmd>>,
}

/// A request, raised by an agent's `spawn_agent` tool, for the swarm to spawn
/// another configured agent. Routed through a channel so tools hold no `Arc`
/// back to the swarm.
pub struct SpawnCmd {
    pub def_id: String,
    pub task: String,
}

/// A running proactive (scheduled) agent's loop parameters.
#[derive(Clone)]
struct ScheduleEntry {
    interval: Duration,
    task: String,
}

/// Signature of a def's scheduling-relevant config, used to detect changes on
/// resync (so unrelated CRUD doesn't needlessly restart agents).
fn def_sig(def: &AgentDef) -> String {
    format!(
        "{:?}",
        (
            def.schedule_mins,
            &def.task,
            &def.instructions,
            &def.tools,
            &def.model,
            def.memory_window,
            def.max_turns,
            def.policy,
        )
    )
}

/// Tracks the suggestion currently being implemented by the developer agent.
#[derive(Clone)]
struct ImplState {
    id: String,
    title: String,
    attempt: usize,
}

pub struct Swarm {
    runtime: Arc<SingleThreadedRuntime>,
    llm: Arc<dyn LLMProvider>,
    model: String,
    ollama_url: String,
    agents: RwLock<HashMap<String, AgentEntry>>,
    sub_index: RwLock<HashMap<SubmissionId, String>>,
    events_tx: broadcast::Sender<SwarmEvent>,
    next_id: AtomicU64,
    /// `Some` ⇒ the research pipeline is enabled; value is the loop interval.
    research_interval: Option<Duration>,
    max_attempts: usize,
    impl_state: Mutex<Option<ImplState>>,
    accept_queue: Mutex<VecDeque<String>>,
    /// Sender for `spawn_agent` tool requests (drained by `spawn_pump`).
    spawn_tx: mpsc::UnboundedSender<SpawnCmd>,
    /// Running proactive agents, keyed by agent id (for re-arming on completion).
    scheduled: Mutex<HashMap<String, ScheduleEntry>>,
    /// Maps a scheduled def id → (running agent id, config signature).
    sched_index: Mutex<HashMap<String, (String, String)>>,
    _env: Mutex<Environment>,
}

impl Swarm {
    /// Build the swarm (Ollama provider), start the runtime + event pump, and —
    /// when `research_interval` is `Some` — the three-agent pipeline.
    pub async fn new(
        model: &str,
        ollama_url: &str,
        research_interval: Option<Duration>,
        max_attempts: usize,
    ) -> Result<Arc<Self>> {
        let llm: Arc<dyn LLMProvider> = LLMBuilder::<autoagents::llm::backends::ollama::Ollama>::new()
            .base_url(ollama_url)
            .model(model)
            .build()
            .wrap_err("failed to build Ollama provider")?;

        let runtime = SingleThreadedRuntime::new(None);
        let mut env = Environment::new(None);
        env.register_runtime(runtime.clone())
            .await
            .wrap_err("failed to register runtime")?;
        let event_stream = env
            .subscribe_events(None)
            .await
            .wrap_err("failed to subscribe to runtime events")?;
        let (events_tx, _) = broadcast::channel(1024);
        let (spawn_tx, spawn_rx) = mpsc::unbounded_channel();
        let _handle = env.run();

        let swarm = Arc::new(Self {
            runtime,
            llm,
            model: model.to_string(),
            ollama_url: ollama_url.to_string(),
            agents: RwLock::new(HashMap::new()),
            sub_index: RwLock::new(HashMap::new()),
            events_tx,
            next_id: AtomicU64::new(1),
            research_interval,
            max_attempts,
            impl_state: Mutex::new(None),
            accept_queue: Mutex::new(VecDeque::new()),
            spawn_tx,
            scheduled: Mutex::new(HashMap::new()),
            sched_index: Mutex::new(HashMap::new()),
            _env: Mutex::new(env),
        });

        swarm.clone().spawn_event_pump(event_stream);
        swarm.clone().spawn_pump(spawn_rx);

        if research_interval.is_some() {
            swarm
                .spawn_system_agents()
                .await
                .wrap_err("failed to start research pipeline")?;
        }
        // Start any proactive (scheduled) user agents saved in the DB.
        swarm.resync_schedules().await;
        Ok(swarm)
    }

    /// Drain `spawn_agent` tool requests and spawn the requested defs.
    fn spawn_pump(self: Arc<Self>, mut rx: mpsc::UnboundedReceiver<SpawnCmd>) {
        tokio::spawn(async move {
            while let Some(cmd) = rx.recv().await {
                if let Ok(Some(def)) = teams::resolve_def(&cmd.def_id) {
                    let _ = self.spawn_from_def(def, cmd.task).await;
                }
            }
        });
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SwarmEvent> {
        self.events_tx.subscribe()
    }

    pub async fn list(&self) -> Vec<AgentInfo> {
        self.agents.read().await.values().map(|e| e.info.clone()).collect()
    }

    pub async fn get(&self, id: &str) -> Option<AgentInfo> {
        self.agents.read().await.get(id).map(|e| e.info.clone())
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn ollama_url(&self) -> &str {
        &self.ollama_url
    }

    // -- agent construction ------------------------------------------------

    /// Build and register an actor from a fully-resolved spec (does not publish a
    /// task). Shared by the system pipeline and configured user agents.
    async fn build_agent(&self, spec: AgentSpec) -> Result<()> {
        let control = AgentControl::new(spec.policy);
        let coding = match spec.prompt {
            Some(prompt) => {
                CodingAgent::coder(&spec.id, control.clone(), prompt, spec.allowed_tools, spec.spawn_tx)
            }
            None => CodingAgent::with_role(&spec.id, spec.role, control.clone()),
        };
        let agent = ReActAgent::with_max_turns(coding, spec.max_turns);
        let topic = Topic::<Task>::new(&spec.id);
        let memory = Box::new(SlidingWindowMemory::new(spec.memory_window));
        let _ = AgentBuilder::<_, ActorAgent>::new(agent)
            .llm(spec.llm)
            .runtime(self.runtime.clone())
            .subscribe(topic)
            .memory(memory)
            .build()
            .await
            .wrap_err("failed to build agent")?;
        let info = AgentInfo {
            id: spec.id.clone(),
            task: String::new(),
            model: spec.model_label,
            policy: spec.policy,
            status: AgentStatus::Queued,
            role: spec.role,
            perpetual: spec.perpetual,
            def_name: spec.def_name,
        };
        self.agents
            .write()
            .await
            .insert(spec.id, AgentEntry { info, control });
        Ok(())
    }

    /// Build a system-pipeline agent (role-fixed prompt/tools, default window &
    /// turns, the shared default model).
    async fn build_system_agent(&self, id: &str, role: Role) -> Result<()> {
        self.build_agent(AgentSpec {
            id: id.to_string(),
            role,
            policy: ApprovalPolicy::AutoApprove,
            llm: self.llm.clone(),
            prompt: None,
            allowed_tools: None,
            memory_window: 20,
            max_turns: 10,
            model_label: self.model.clone(),
            perpetual: true,
            def_name: None,
            spawn_tx: None,
        })
        .await
    }

    /// Resolve the LLM provider for a model: reuse the shared one when the model
    /// matches, otherwise build a per-agent Ollama provider.
    fn provider_for(&self, model: &str) -> Result<Arc<dyn LLMProvider>> {
        if model == self.model {
            Ok(self.llm.clone())
        } else {
            let p: Arc<dyn LLMProvider> =
                LLMBuilder::<autoagents::llm::backends::ollama::Ollama>::new()
                    .base_url(&self.ollama_url)
                    .model(model)
                    .build()
                    .wrap_err_with(|| format!("failed to build provider for model `{model}`"))?;
            Ok(p)
        }
    }

    /// Publish a task to an existing agent, mapping its submission id.
    async fn publish_to(&self, id: &str, task_text: String) -> Result<()> {
        let task = Task::new(task_text.clone());
        let sub_id = task.submission_id;
        if let Some(e) = self.agents.write().await.get_mut(id) {
            e.info.task = task_text;
            e.info.status = AgentStatus::Queued;
        }
        self.sub_index.write().await.insert(sub_id, id.to_string());
        self.runtime
            .publish(&Topic::<Task>::new(id), task)
            .await
            .wrap_err("failed to publish task")?;
        Ok(())
    }

    /// Spawn a user agent from a saved definition and give it a task.
    pub async fn spawn_from_def(&self, def: AgentDef, task: String) -> Result<String> {
        let id = format!("agent-{}", self.next_id.fetch_add(1, Ordering::SeqCst));
        let model = def.model.clone().unwrap_or_else(|| self.model.clone());
        let llm = self.provider_for(&model)?;
        self.build_agent(AgentSpec {
            id: id.clone(),
            role: Role::Coder,
            policy: def.policy,
            llm,
            prompt: Some(def.instructions),
            allowed_tools: Some(def.tools),
            memory_window: def.memory_window.unwrap_or(20),
            max_turns: def.max_turns.unwrap_or(10),
            model_label: model,
            perpetual: false,
            def_name: Some(def.name),
            spawn_tx: Some(self.spawn_tx.clone()),
        })
        .await?;
        self.publish_to(&id, task).await?;
        Ok(id)
    }

    // -- proactive (scheduled) agents --------------------------------------

    /// Reconcile running scheduled agents with the saved definitions: start newly
    /// scheduled defs, stop ones that are gone/disabled, and restart changed ones.
    pub async fn resync_schedules(self: &Arc<Self>) {
        // Desired: user defs with a positive interval and a non-empty recurring task.
        let mut desired: HashMap<String, AgentDef> = HashMap::new();
        if let Ok(teams) = teams::all_teams() {
            for tw in teams {
                for def in tw.agents {
                    let has_task = def.task.as_deref().map(|t| !t.trim().is_empty()).unwrap_or(false);
                    if !def.builtin && def.schedule_mins.unwrap_or(0) > 0 && has_task {
                        desired.insert(def.id.clone(), def);
                    }
                }
            }
        }
        // Stop schedules that are gone or whose config changed (changed ones are
        // re-created below).
        let current: Vec<(String, String)> = self
            .sched_index
            .lock()
            .await
            .iter()
            .map(|(def_id, (_, sig))| (def_id.clone(), sig.clone()))
            .collect();
        for (def_id, sig) in current {
            let keep = desired.get(&def_id).map(|d| def_sig(d) == sig).unwrap_or(false);
            if !keep {
                self.remove_schedule(&def_id).await;
            }
        }
        // Start any desired schedule not currently running.
        for (def_id, def) in desired {
            let running = self.sched_index.lock().await.contains_key(&def_id);
            if !running {
                let _ = self.start_schedule(def).await;
            }
        }
    }

    /// Build and launch one proactive agent for `def`, and register its loop.
    async fn start_schedule(self: &Arc<Self>, def: AgentDef) -> Result<()> {
        let Some(mins) = def.schedule_mins else {
            return Ok(());
        };
        let task = def.task.clone().unwrap_or_default();
        if mins == 0 || task.trim().is_empty() {
            return Ok(());
        }
        let interval = Duration::from_secs(mins * 60);
        let agent_id = format!("sched-{}-{}", def.id, self.next_id.fetch_add(1, Ordering::SeqCst));
        let model = def.model.clone().unwrap_or_else(|| self.model.clone());
        let llm = self.provider_for(&model)?;
        self.build_agent(AgentSpec {
            id: agent_id.clone(),
            role: Role::Coder,
            policy: def.policy,
            llm,
            prompt: Some(def.instructions.clone()),
            allowed_tools: Some(def.tools.clone()),
            memory_window: def.memory_window.unwrap_or(20),
            max_turns: def.max_turns.unwrap_or(10),
            model_label: model,
            perpetual: true,
            def_name: Some(def.name.clone()),
            spawn_tx: Some(self.spawn_tx.clone()),
        })
        .await?;
        self.scheduled
            .lock()
            .await
            .insert(agent_id.clone(), ScheduleEntry { interval, task: task.clone() });
        self.sched_index
            .lock()
            .await
            .insert(def.id.clone(), (agent_id.clone(), def_sig(&def)));
        self.publish_to(&agent_id, task).await?;
        Ok(())
    }

    /// Stop and deregister the proactive agent for `def_id` (if any).
    async fn remove_schedule(&self, def_id: &str) {
        let Some((agent_id, _)) = self.sched_index.lock().await.remove(def_id) else {
            return;
        };
        self.scheduled.lock().await.remove(&agent_id);
        if let Some(entry) = self.agents.write().await.get_mut(&agent_id) {
            entry.control.cancel();
            entry.info.status = AgentStatus::Cancelled;
        }
    }

    async fn spawn_system_agents(&self) -> Result<()> {
        self.build_system_agent(RESEARCHER_ID, Role::Researcher).await?;
        self.build_system_agent(PLANNER_ID, Role::Planner).await?;
        self.build_system_agent(DEVELOPER_ID, Role::Developer).await?;
        self.publish_to(RESEARCHER_ID, RESEARCH_TASK.to_string()).await?;
        Ok(())
    }

    // -- cancellation ------------------------------------------------------

    pub async fn cancel(&self, id: &str) -> bool {
        if let Some(entry) = self.agents.write().await.get_mut(id) {
            entry.control.cancel();
            entry.info.status = AgentStatus::Cancelled;
            true
        } else {
            false
        }
    }

    /// Cancel every *user* agent; the built-in system agents keep running.
    pub async fn cancel_all(&self) {
        for entry in self.agents.write().await.values_mut() {
            if entry.info.role != Role::Coder {
                continue;
            }
            entry.control.cancel();
            if matches!(entry.info.status, AgentStatus::Queued | AgentStatus::Running) {
                entry.info.status = AgentStatus::Cancelled;
            }
        }
    }

    // -- task review actions (called by the server) ------------------------

    /// Accept a task: start (or queue) its implementation by the developer.
    pub async fn accept(&self, id: &str) -> Result<()> {
        store::set_status(id, "accepted")?;
        let idle = self.impl_state.lock().await.is_none();
        if idle {
            self.start_implementation(id).await?;
        } else {
            self.accept_queue.lock().await.push_back(id.to_string());
        }
        Ok(())
    }

    pub async fn reject(&self, id: &str) -> Result<()> {
        store::set_status(id, "rejected")
    }

    /// Begin work on an accepted task: the planner drafts an implementation plan
    /// (the developer is triggered later, in `on_planner_done`).
    async fn start_implementation(&self, id: &str) -> Result<()> {
        let task = store::get(id)?;
        *self.impl_state.lock().await = Some(ImplState {
            id: id.to_string(),
            title: task.title.clone(),
            attempt: 1,
        });
        let prompt = format!(
            "Plan the implementation of this APPROVED frontend task.\n\n# {}\n{}",
            task.title, task.body
        );
        self.publish_to(PLANNER_ID, prompt).await?;
        Ok(())
    }

    // -- pipeline event handling ------------------------------------------

    fn spawn_event_pump(
        self: Arc<Self>,
        mut stream: autoagents::core::utils::BoxEventStream<Event>,
    ) {
        tokio::spawn(async move {
            while let Some(event) = stream.next().await {
                let Some(sub_id) = event_sub_id(&event) else {
                    continue;
                };
                let agent_id = {
                    let idx = self.sub_index.read().await;
                    match idx.get(&sub_id) {
                        Some(id) => id.clone(),
                        None => continue,
                    }
                };

                if let Some(status) = event_status(&event)
                    && let Some(entry) = self.agents.write().await.get_mut(&agent_id)
                    && entry.info.status != AgentStatus::Cancelled
                {
                    // Perpetual system agents read as Idle (alive, waiting) rather
                    // than Done when a cycle finishes.
                    entry.info.status = if entry.info.perpetual && status == AgentStatus::Done {
                        AgentStatus::Idle
                    } else {
                        status
                    };
                }

                if let Ok(value) = serde_json::to_value(&event) {
                    let _ = self.events_tx.send(SwarmEvent {
                        agent_id: agent_id.clone(),
                        event: value,
                    });
                }

                // Pipeline handoffs on task completion.
                if let Some(result) = event_complete_result(&event) {
                    match agent_id.as_str() {
                        // Researcher adds tasks itself, then loops on its own cadence —
                        // it does NOT trigger the planner.
                        RESEARCHER_ID => self.on_researcher_done(),
                        // Planner only runs after a human accept; its plan goes to the developer.
                        PLANNER_ID => self.on_planner_done(result).await,
                        DEVELOPER_ID => self.clone().spawn_finish_developer(result),
                        // A proactive user agent finished a cycle: refresh the queue
                        // (it may have added tasks) and re-arm after its interval.
                        _ => self.on_scheduled_done(&agent_id).await,
                    }
                }

                // Proactive agents also re-arm after a failed cycle, so a transient
                // error doesn't permanently stop the schedule.
                if matches!(event_status(&event), Some(AgentStatus::Error)) {
                    self.on_scheduled_done(&agent_id).await;
                }
            }
        });
    }

    /// A proactive (scheduled) user agent finished a cycle: refresh the task
    /// queue (it may have enqueued work for review) and re-publish its recurring
    /// task after the configured interval, unless it has since been cancelled or
    /// rescheduled.
    async fn on_scheduled_done(self: &Arc<Self>, agent_id: &str) {
        let Some(entry) = self.scheduled.lock().await.get(agent_id).cloned() else {
            return; // not a scheduled agent
        };
        self.broadcast_control("TasksChanged");
        let me = Arc::clone(self);
        let id = agent_id.to_string();
        tokio::spawn(async move {
            tokio::time::sleep(entry.interval).await;
            let cancelled = me
                .agents
                .read()
                .await
                .get(&id)
                .map(|e| e.control.is_cancelled())
                .unwrap_or(true);
            let still_scheduled = me.scheduled.lock().await.contains_key(&id);
            if !cancelled && still_scheduled {
                let _ = me.publish_to(&id, entry.task).await;
            }
        });
    }

    /// Researcher finished a cycle (and added tasks via `add_task`): refresh the
    /// dashboard and schedule the next research cycle. The planner is NOT involved.
    fn on_researcher_done(self: &Arc<Self>) {
        self.broadcast_control("TasksChanged");
        if let Some(interval) = self.research_interval {
            let me = Arc::clone(self);
            tokio::spawn(async move {
                tokio::time::sleep(interval).await;
                let cancelled = me
                    .agents
                    .read()
                    .await
                    .get(RESEARCHER_ID)
                    .map(|e| e.control.is_cancelled())
                    .unwrap_or(true);
                if !cancelled {
                    let _ = me.publish_to(RESEARCHER_ID, RESEARCH_TASK.to_string()).await;
                }
            });
        }
    }

    /// Planner produced an implementation plan for the in-flight accepted task;
    /// hand it (with the task body) to the developer.
    async fn on_planner_done(&self, plan: String) {
        let Some(state) = self.impl_state.lock().await.clone() else {
            return;
        };
        let _ = store::set_status(&state.id, "implementing");
        let body = store::get(&state.id).map(|t| t.body).unwrap_or_default();
        let prompt = format!(
            "Implement this approved frontend task by editing files under `frontend/`.\n\n# Task\n{body}\n\n# Plan\n{plan}"
        );
        let _ = self.publish_to(DEVELOPER_ID, prompt).await;
        self.broadcast_control("TasksChanged");
    }

    fn spawn_finish_developer(self: Arc<Self>, dev_result: String) {
        tokio::spawn(async move { self.finish_developer(dev_result).await });
    }

    /// After the developer reports done: build; on success commit to a branch
    /// and tell the browser to reload; on failure retry (capped) or fail.
    async fn finish_developer(self: Arc<Self>, dev_result: String) {
        let Some(state) = self.impl_state.lock().await.clone() else {
            return;
        };

        match trunk_build().await {
            Ok(()) => {
                let _ = git_commit_branch(state.id.clone(), state.title.clone()).await;
                let _ = store::set_status(&state.id, "implemented");
                append_note(
                    &state.id,
                    "Implementation",
                    &format!("Committed to branch `okf/{}`.\n\n{}", state.id, dev_result),
                );
                self.broadcast_control("TasksChanged");
                self.broadcast_control("Reload");
                self.finish_and_dequeue().await;
            }
            Err(stderr) if state.attempt < self.max_attempts => {
                if let Some(s) = self.impl_state.lock().await.as_mut() {
                    s.attempt += 1;
                }
                let task = format!(
                    "Build failed (attempt {}/{}):\n\n{}\n\nFix these errors in frontend/ and continue.",
                    state.attempt + 1,
                    self.max_attempts,
                    truncate(&stderr, 4000)
                );
                let _ = self.publish_to(DEVELOPER_ID, task).await;
            }
            Err(stderr) => {
                let _ = store::set_status(&state.id, "failed");
                append_note(&state.id, "Build error", &truncate(&stderr, 8000));
                git_discard_frontend().await;
                self.broadcast_control("TasksChanged");
                self.finish_and_dequeue().await;
            }
        }
    }

    async fn finish_and_dequeue(self: &Arc<Self>) {
        *self.impl_state.lock().await = None;
        let next = self.accept_queue.lock().await.pop_front();
        if let Some(id) = next {
            let _ = self.start_implementation(&id).await;
        }
    }

    /// Send a control frame (e.g. `Reload`, `OkfChanged`) to dashboard clients.
    fn broadcast_control(&self, kind: &str) {
        let mut map = serde_json::Map::new();
        map.insert(kind.to_string(), serde_json::json!({}));
        let _ = self.events_tx.send(SwarmEvent {
            agent_id: "system".to_string(),
            event: serde_json::Value::Object(map),
        });
    }
}

// -- build / git helpers (blocking, run off the runtime) -------------------

async fn trunk_build() -> std::result::Result<(), String> {
    tokio::task::spawn_blocking(|| {
        let out = std::process::Command::new("trunk")
            .arg("build")
            .current_dir("frontend")
            .output()
            .map_err(|e| format!("failed to run trunk: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).to_string())
        }
    })
    .await
    .unwrap_or_else(|e| Err(format!("build task panicked: {e}")))
}

/// Commit the frontend changes to a fresh `okf/<id>` branch, then return to the
/// original branch (leaving the working branch clean).
async fn git_commit_branch(id: String, title: String) -> std::result::Result<(), String> {
    tokio::task::spawn_blocking(move || {
        let run = |args: &[&str]| -> std::result::Result<(), String> {
            let out = std::process::Command::new("git")
                .args(args)
                .output()
                .map_err(|e| format!("git {args:?}: {e}"))?;
            if out.status.success() {
                Ok(())
            } else {
                Err(String::from_utf8_lossy(&out.stderr).to_string())
            }
        };
        let orig_out = std::process::Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .output()
            .map_err(|e| e.to_string())?;
        let orig = String::from_utf8_lossy(&orig_out.stdout).trim().to_string();
        let branch = format!("okf/{id}");
        run(&["checkout", "-b", &branch])?;
        run(&["add", "--", "frontend"])?;
        run(&["commit", "-m", &format!("okf {id}: {title}")])?;
        run(&["checkout", &orig])?;
        Ok(())
    })
    .await
    .unwrap_or_else(|e| Err(format!("git task panicked: {e}")))
}

async fn git_discard_frontend() {
    let _ = tokio::task::spawn_blocking(|| {
        let _ = std::process::Command::new("git")
            .args(["checkout", "--", "frontend"])
            .output();
    })
    .await;
}

/// Append a `## heading` note to a task's markdown body (best-effort).
fn append_note(id: &str, heading: &str, text: &str) {
    if let Ok(t) = store::get(id) {
        let body = format!("{}\n\n## {heading}\n{text}", t.body);
        let _ = store::update(id, &t.title, &body);
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() > n {
        format!("{}…", &s[..n])
    } else {
        s.to_string()
    }
}

fn event_sub_id(event: &Event) -> Option<SubmissionId> {
    match event {
        Event::TaskStarted { sub_id, .. }
        | Event::TaskComplete { sub_id, .. }
        | Event::TaskError { sub_id, .. }
        | Event::ToolCallRequested { sub_id, .. }
        | Event::ToolCallCompleted { sub_id, .. }
        | Event::ToolCallFailed { sub_id, .. }
        | Event::TurnStarted { sub_id, .. }
        | Event::TurnCompleted { sub_id, .. }
        | Event::StreamChunk { sub_id, .. }
        | Event::StreamToolCall { sub_id, .. }
        | Event::StreamComplete { sub_id, .. } => Some(*sub_id),
        _ => None,
    }
}

fn event_status(event: &Event) -> Option<AgentStatus> {
    match event {
        Event::TaskStarted { .. } => Some(AgentStatus::Running),
        Event::TaskComplete { .. } => Some(AgentStatus::Done),
        Event::TaskError { .. } => Some(AgentStatus::Error),
        _ => None,
    }
}

fn event_complete_result(event: &Event) -> Option<String> {
    match event {
        Event::TaskComplete { result, .. } => Some(result.clone()),
        _ => None,
    }
}
