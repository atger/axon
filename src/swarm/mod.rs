//! Swarm orchestration on AutoAgents: a registry of concurrent agents driven by
//! a shared runtime and a protocol-event pump.

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
use autoagents::llm::completion::CompletionRequest;
use autoagents::protocol::{Event, SubmissionId};
use color_eyre::eyre::{Result, WrapErr};
use serde::Serialize;
use tokio::sync::{Mutex, RwLock, broadcast, mpsc};
use tokio_stream::StreamExt;

use agent::{AgentControl, ApprovalPolicy, CodingAgent, Role};
use teams::AgentDef;



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
    /// ISO-8601 timestamp when the agent was started.
    #[serde(default)]
    pub started: String,
    /// ISO-8601 timestamp when the current cycle started (for scheduled/perpetual
    /// agents); updated each time a new task is published.
    #[serde(default)]
    pub cycle_started: String,
    /// ISO-8601 timestamp when the current cycle completed (for scheduled/perpetual
    /// agents); empty string while the cycle is still running.
    #[serde(default)]
    pub cycle_completed: String,
}

struct AgentEntry {
    info: AgentInfo,
    control: Arc<AgentControl>,
}

#[derive(Clone, Serialize)]
pub struct HistoricAgent {
    pub id: String,
    pub task: String,
    pub model: String,
    pub status: AgentStatus,
    pub def_name: Option<String>,
    pub started: String,
    pub completed: String,
    pub result: Option<String>,
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

pub struct Swarm {
    runtime: Arc<SingleThreadedRuntime>,
    llm: RwLock<Arc<dyn LLMProvider>>,
    model: RwLock<String>,
    ollama_url: String,
    agents: RwLock<HashMap<String, AgentEntry>>,
    sub_index: RwLock<HashMap<SubmissionId, String>>,
    events_tx: broadcast::Sender<SwarmEvent>,
    next_id: AtomicU64,
    /// Sender for `spawn_agent` tool requests (drained by `spawn_pump`).
    spawn_tx: mpsc::UnboundedSender<SpawnCmd>,
    /// Running proactive agents, keyed by agent id (for re-arming on completion).
    scheduled: Mutex<HashMap<String, ScheduleEntry>>,
    /// Maps a scheduled def id → (running agent id, config signature).
    sched_index: Mutex<HashMap<String, (String, String)>>,
    agent_history: Mutex<VecDeque<HistoricAgent>>,
    _env: Mutex<Environment>,
}

impl Swarm {
    /// Build the swarm (Ollama provider), start the runtime + event pump.
    pub async fn new(model: &str, ollama_url: &str) -> Result<Arc<Self>> {
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
            llm: RwLock::new(llm),
            model: RwLock::new(model.to_string()),
            ollama_url: ollama_url.to_string(),
            agents: RwLock::new(HashMap::new()),
            sub_index: RwLock::new(HashMap::new()),
            events_tx,
            next_id: AtomicU64::new(1),
            spawn_tx,
            scheduled: Mutex::new(HashMap::new()),
            sched_index: Mutex::new(HashMap::new()),
            agent_history: Mutex::new(VecDeque::new()),
            _env: Mutex::new(env),
        });

        swarm.clone().spawn_event_pump(event_stream);
        swarm.clone().spawn_pump(spawn_rx);

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

    pub async fn history(&self) -> Vec<HistoricAgent> {
        self.agent_history.lock().await.iter().cloned().collect()
    }

    pub async fn model(&self) -> String {
        self.model.read().await.clone()
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
        let now = chrono::Local::now().to_rfc3339();
        let info = AgentInfo {
            id: spec.id.clone(),
            task: String::new(),
            model: spec.model_label,
            policy: spec.policy,
            status: AgentStatus::Queued,
            role: spec.role,
            perpetual: spec.perpetual,
            def_name: spec.def_name,
            started: now.clone(),
            cycle_started: now.clone(),
            cycle_completed: String::new(),
        };
        self.agents
            .write()
            .await
            .insert(spec.id, AgentEntry { info, control });
        Ok(())
    }

    /// Resolve the LLM provider for a model: reuse the shared one when the model
    /// matches, otherwise build a per-agent Ollama provider.
    async fn provider_for(&self, model: &str) -> Result<Arc<dyn LLMProvider>> {
        let current = self.model.read().await;
        if model == *current {
            Ok(self.llm.read().await.clone())
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
        eprintln!("[swarm] publish_to: id={id} sub_id={sub_id} task_len={}", task_text.len());
        if let Some(e) = self.agents.write().await.get_mut(id) {
            e.info.task = task_text;
            e.info.status = AgentStatus::Queued;
            e.info.cycle_started = chrono::Local::now().to_rfc3339();
            e.info.cycle_completed = String::new();
            eprintln!("[swarm] publish_to: found agent, set status=Queued");
        } else {
            eprintln!("[swarm] publish_to: agent {id} NOT found in agents map");
        }
        self.sub_index.write().await.insert(sub_id, id.to_string());
        self.runtime
            .publish(&Topic::<Task>::new(id), task)
            .await
            .wrap_err("failed to publish task")?;
        Ok(())
    }

    /// Spawn a user agent from a saved definition and give it a task.
    /// When the definition has a non-zero `schedule_mins` and a recurring
    /// `task`, this delegates to the proactive scheduling system instead so the
    /// agent re-runs on its configured interval.
    pub async fn spawn_from_def(self: &Arc<Self>, def: AgentDef, task: String) -> Result<String> {
        // If the def is configured for proactive scheduling, route through the
        // schedule system (perpetual, auto-re-arming) instead of a one-shot.
        let has_schedule = def.schedule_mins.unwrap_or(0) > 0;
        eprintln!(
            "[swarm] spawn_from_def: id={} name={} schedule_mins={:?} task={:?} has_schedule={has_schedule}",
            def.id, def.name, def.schedule_mins, def.task,
        );
        if has_schedule {
            // Use the Spawn-provided task as the recurring task override so
            // the agent works even when the def hasn't pre-configured a task.
            return self.start_schedule(def, Some(task)).await;
        }

        let id = format!("agent-{}", self.next_id.fetch_add(1, Ordering::SeqCst));
        let default = self.model.read().await.clone();
        let model = def.model.clone().unwrap_or(default);
        let llm = self.provider_for(&model).await?;
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
                        eprintln!(
                            "[swarm] resync: found scheduled def id={} name={} mins={} task={}",
                            def.id, def.name, def.schedule_mins.unwrap_or(0),
                            def.task.as_deref().unwrap_or("")
                        );
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
                eprintln!("[swarm] resync: removing schedule for def_id={def_id}");
                self.remove_schedule(&def_id).await;
            }
        }
        // Start any desired schedule not currently running.
        for (def_id, def) in desired {
            let running = self.sched_index.lock().await.contains_key(&def_id);
            if !running {
                match self.start_schedule(def, None).await {
                    Ok(aid) => eprintln!("[swarm] resync: started schedule agent_id={aid} for def_id={def_id}"),
                    Err(e) => eprintln!("[swarm] resync: failed to start schedule for def_id={def_id}: {e}"),
                }
            } else {
                eprintln!("[swarm] resync: schedule already running for def_id={def_id}");
            }
        }
    }

    /// Build and launch one proactive agent for `def`, and register its loop.
    /// When `task_override` is provided (non-empty), it is used as the recurring
    /// task instead of `def.task`. This lets the Spawn API supply the recurring
    /// task when the user hasn't pre-configured one on the def.
    /// Returns the spawned agent id (e.g. `sched-{def_id}-{N}`).
    async fn start_schedule(self: &Arc<Self>, def: AgentDef, task_override: Option<String>) -> Result<String> {
        let Some(mins) = def.schedule_mins else {
            return Err(color_eyre::eyre::eyre!("start_schedule: no schedule_mins"));
        };
        let task = task_override
            .filter(|t| !t.trim().is_empty())
            .or_else(|| def.task.clone())
            .unwrap_or_default();
        if mins == 0 || task.trim().is_empty() {
            return Err(color_eyre::eyre::eyre!("start_schedule: mins=0 or task empty"));
        }
        let interval = Duration::from_secs(mins * 60);
        let agent_id = format!("sched-{}-{}", def.id, self.next_id.fetch_add(1, Ordering::SeqCst));
        let default = self.model.read().await.clone();
        let model = def.model.clone().unwrap_or(default);
        let llm = self.provider_for(&model).await?;
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
        eprintln!("[swarm] start_schedule: agent_id={agent_id} interval={mins}min publishing first task");
        self.publish_to(&agent_id, task).await?;
        Ok(agent_id)
    }

    /// Stop and deregister the proactive agent for `def_id` (if any).
    async fn remove_schedule(&self, def_id: &str) {
        let Some((agent_id, _)) = self.sched_index.lock().await.remove(def_id) else {
            return;
        };
        eprintln!("[swarm] remove_schedule: def_id={def_id} agent_id={agent_id}");
        self.scheduled.lock().await.remove(&agent_id);
        if let Some(entry) = self.agents.write().await.get_mut(&agent_id) {
            entry.control.cancel();
            entry.info.status = AgentStatus::Cancelled;
        }
    }

    // -- cancellation ------------------------------------------------------

    pub async fn cancel(&self, id: &str) -> bool {
        if let Some(entry) = self.agents.write().await.get_mut(id) {
            if entry.info.perpetual && entry.info.status == AgentStatus::Idle {
                return true;
            }
            entry.control.cancel();
            entry.info.status = AgentStatus::Cancelled;
            true
        } else {
            false
        }
    }

    /// Cancel every running agent (skips perpetual idle agents between cycles).
    pub async fn cancel_all(&self) {
        for entry in self.agents.write().await.values_mut() {
            if entry.info.perpetual && entry.info.status == AgentStatus::Idle {
                continue;
            }
            entry.control.cancel();
            if matches!(entry.info.status, AgentStatus::Queued | AgentStatus::Running) {
                entry.info.status = AgentStatus::Cancelled;
            }
        }
    }

    /// Generate an agent definition template from a user's natural-language
    /// description, using the LLM. Returns markdown with YAML frontmatter.
    pub async fn generate_agent_def(&self, user_prompt: &str) -> Result<String> {
        let system = "\
You are an expert at designing AI agent configurations for the Axon swarm system.
Output raw markdown only. Do NOT wrap the output in any JSON, envelope, or structured format.
Use this exact format, starting with --- and ending with the markdown body:

---
name: <agent-name>
model:  # leave empty to use default
tools:
  - write_file
  - add_task
  # - run_command
  # - web_search
policy: auto_approve
memory_window: 20
max_turns: 10
schedule_mins: 15
task:
task_hint:
---

# Agent instructions

<what this agent does>

When done, call add_task(title=<name>, body=<markdown>) to submit your work for human review (body must be markdown).

Based on the user's description, generate a complete agent definition. Choose appropriate tools, policy, \
and write clear instructions. If relevant, include schedule_mins and task. Output raw markdown only — \
no JSON, no commentary.";

        let prompt = format!("{system}\n\nUser request: {user_prompt}");

        let req = CompletionRequest::builder(prompt)
            .max_tokens(2000)
            .temperature(0.7)
            .build();
        let resp = self
            .llm
            .read()
            .await
            .complete(&req, None)
            .await
            .wrap_err("LLM completion failed")?;
        Ok(strip_json_envelope(&resp.text))
    }

    // -- event handling ---------------------------------------------------

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
                    if matches!(status, AgentStatus::Done | AgentStatus::Error) {
                        entry.info.cycle_completed = chrono::Local::now().to_rfc3339();
                    }
                }

                if let Ok(value) = serde_json::to_value(&event) {
                    let _ = self.events_tx.send(SwarmEvent {
                        agent_id: agent_id.clone(),
                        event: value,
                    });
                }

                // On completion/error: record history, create review tasks,
                // and re-arm proactive agents.
                let is_complete = event_complete_result(&event).is_some();
                let is_error = matches!(event_status(&event), Some(AgentStatus::Error));
                if is_complete || is_error {
                    let now = chrono::Local::now().to_rfc3339();
                    let info = self.agents.read().await.get(&agent_id).map(|e| e.info.clone());
                    if let Some(info) = info {
                        let result = event_complete_result(&event);
                        eprintln!(
                            "[swarm] event_pump: agent={agent_id} status={:?} perpetual={} is_complete={is_complete} is_error={is_error}",
                            info.status, info.perpetual,
                        );
                        self.agent_history.lock().await.push_front(HistoricAgent {
                            id: agent_id.clone(),
                            task: info.task.clone(),
                            model: info.model.clone(),
                            status: info.status,
                            def_name: info.def_name.clone(),
                            started: info.started.clone(),
                            completed: now,
                            result: result.clone(),
                        });
                        self.prune_history();

                        // Auto-create a review task for non-perpetual agents.
                        if !info.perpetual && is_complete {
                            if let Some(ref res) = result {
                                let title = if info.task.is_empty() {
                                    format!("Agent {} completed", &agent_id)
                                } else {
                                    info.task.clone()
                                };
                                let body = strip_json_envelope(res);
                                let _ = store::add_task(
                                    &title,
                                    &format!("Auto-created from agent `{}`", &agent_id),
                                    "agent",
                                    &body,
                                );
                                self.broadcast_control("TasksChanged");
                            }
                        }
                    } else {
                        eprintln!("[swarm] event_pump: agent={agent_id} completed but NOT found in agents map");
                    }
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
            eprintln!("[swarm] on_scheduled_done: agent={agent_id} NOT in scheduled map (not a scheduled agent)");
            return;
        };
        eprintln!(
            "[swarm] on_scheduled_done: agent={agent_id} scheduling next cycle in {}s",
            entry.interval.as_secs()
        );
        self.broadcast_control("TasksChanged");
        let me = Arc::clone(self);
        let id = agent_id.to_string();

        // Align next publish to the schedule boundary (cycle_started + interval)
        // rather than sleeping a full interval from completion time.
        let cs_str = self.agents.read().await.get(agent_id)
            .map(|e| e.info.cycle_started.clone())
            .unwrap_or_default();
        let sleep_dur = if cs_str.is_empty() {
            entry.interval
        } else if let Ok(cs) = chrono::DateTime::parse_from_rfc3339(&cs_str) {
            let cs_utc = cs.with_timezone(&chrono::Utc);
            let elapsed = (chrono::Utc::now() - cs_utc).to_std().unwrap_or(Duration::ZERO);
            if elapsed < entry.interval {
                entry.interval - elapsed
            } else {
                Duration::ZERO
            }
        } else {
            entry.interval
        };

        tokio::spawn(async move {
            tokio::time::sleep(sleep_dur).await;
            let status = me.agents.read().await.get(&id).map(|e| e.info.status);
            let alive = status.map(|s| s != AgentStatus::Cancelled).unwrap_or(false);
            let still_scheduled = me.scheduled.lock().await.contains_key(&id);
            eprintln!(
                "[swarm] on_scheduled_done: wake agent={id} alive={alive} status={:?} still_scheduled={still_scheduled}",
                status
            );
            if alive && still_scheduled {
                if let Err(e) = me.publish_to(&id, entry.task).await {
                    eprintln!("[swarm] on_scheduled_done: failed to re-publish for {id}: {e}");
                } else {
                    eprintln!("[swarm] on_scheduled_done: re-published task for {id}");
                }
            }
        });
    }

    /// Switch the default model at runtime. Rebuilds the shared Ollama provider.
    pub async fn set_model(&self, new_model: &str) -> Result<()> {
        let provider: Arc<dyn LLMProvider> =
            LLMBuilder::<autoagents::llm::backends::ollama::Ollama>::new()
                .base_url(&self.ollama_url)
                .model(new_model)
                .build()
                .wrap_err_with(|| {
                    format!("failed to build provider for model `{new_model}`")
                })?;
        *self.llm.write().await = provider;
        *self.model.write().await = new_model.to_string();
        self.broadcast_control("ModelChanged");
        Ok(())
    }

    /// Send a control frame (e.g. `Reload`, `TasksChanged`) to dashboard clients.
    fn prune_history(&self) {
        if let Ok(mut h) = self.agent_history.try_lock() {
            while h.len() > 500 {
                h.pop_back();
            }
        }
    }

    pub(crate) fn broadcast_control(&self, kind: &str) {
        let mut map = serde_json::Map::new();
        map.insert(kind.to_string(), serde_json::json!({}));
        let _ = self.events_tx.send(SwarmEvent {
            agent_id: "system".to_string(),
            event: serde_json::Value::Object(map),
        });
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

/// Some Ollama models wrap their response in a JSON envelope
/// `{"done":true,"response":"...","tool_calls":[]}`. Strip it if present.
fn strip_json_envelope(text: &str) -> String {
    let t = text.trim();
    if t.starts_with('{') {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(t) {
            if let Some(resp) = v.get("response").and_then(|r| r.as_str()) {
                return resp.to_string();
            }
        }
    }
    text.to_string()
}
