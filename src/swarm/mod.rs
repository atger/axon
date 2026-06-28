//! Swarm orchestration built on AutoAgents: a registry of concurrently running
//! agents driven by a shared `SingleThreadedRuntime` + `Environment`, with a
//! broadcast of protocol events for the dashboard.

pub mod agent;
pub mod tools;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

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
use tokio::sync::{Mutex, RwLock, broadcast};
use tokio_stream::StreamExt;

use agent::{AgentControl, ApprovalPolicy, CodingAgent};

/// Lifecycle status of a swarm agent, surfaced to the dashboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Queued,
    Running,
    Done,
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
}

struct AgentEntry {
    info: AgentInfo,
    control: Arc<AgentControl>,
}

/// An event forwarded to dashboard WebSocket clients: the raw AutoAgents
/// protocol event, tagged with the axon agent id it belongs to.
#[derive(Clone, Serialize)]
pub struct SwarmEvent {
    pub agent_id: String,
    pub event: serde_json::Value,
}

/// Parameters for spawning a new agent.
pub struct SpawnSpec {
    pub task: String,
    pub policy: ApprovalPolicy,
}

pub struct Swarm {
    runtime: Arc<SingleThreadedRuntime>,
    llm: Arc<dyn LLMProvider>,
    model: String,
    agents: RwLock<HashMap<String, AgentEntry>>,
    /// Maps an AutoAgents submission id to our agent id (events carry sub_id).
    sub_index: RwLock<HashMap<SubmissionId, String>>,
    events_tx: broadcast::Sender<SwarmEvent>,
    next_id: AtomicU64,
    // Kept alive so the runtime keeps running; also used for graceful shutdown.
    _env: Mutex<Environment>,
}

impl Swarm {
    /// Build the swarm with an Ollama-backed local provider and start the
    /// AutoAgents runtime + event pump.
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

        // Non-consuming subscription so the pump can read all protocol events.
        let event_stream = env
            .subscribe_events(None)
            .await
            .wrap_err("failed to subscribe to runtime events")?;

        let (events_tx, _) = broadcast::channel(1024);

        // Start driving the runtime in the background (runs until shutdown).
        let _handle = env.run();

        let swarm = Arc::new(Self {
            runtime,
            llm,
            model: model.to_string(),
            agents: RwLock::new(HashMap::new()),
            sub_index: RwLock::new(HashMap::new()),
            events_tx,
            next_id: AtomicU64::new(1),
            _env: Mutex::new(env),
        });

        swarm.clone().spawn_event_pump(event_stream);
        Ok(swarm)
    }

    /// Subscribe to the swarm-wide event stream (one receiver per WS client).
    pub fn subscribe(&self) -> broadcast::Receiver<SwarmEvent> {
        self.events_tx.subscribe()
    }

    pub async fn list(&self) -> Vec<AgentInfo> {
        self.agents
            .read()
            .await
            .values()
            .map(|e| e.info.clone())
            .collect()
    }

    pub async fn get(&self, id: &str) -> Option<AgentInfo> {
        self.agents.read().await.get(id).map(|e| e.info.clone())
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// Spawn a new agent: build a ReAct actor, subscribe it to a unique topic,
    /// and publish the task. Returns the new agent id.
    pub async fn spawn(&self, spec: SpawnSpec) -> Result<String> {
        let id = format!("agent-{}", self.next_id.fetch_add(1, Ordering::SeqCst));
        let control = AgentControl::new(spec.policy);

        let agent = ReActAgent::new(CodingAgent::new(&id, control.clone()));
        let topic = Topic::<Task>::new(&id);
        let memory = Box::new(SlidingWindowMemory::new(20));

        // build() registers the actor with the shared runtime; the handle can be
        // dropped — the runtime owns the actor for its lifetime.
        let _ = AgentBuilder::<_, ActorAgent>::new(agent)
            .llm(self.llm.clone())
            .runtime(self.runtime.clone())
            .subscribe(topic.clone())
            .memory(memory)
            .build()
            .await
            .wrap_err("failed to build agent")?;

        let task = Task::new(spec.task.clone());
        let sub_id = task.submission_id;

        let info = AgentInfo {
            id: id.clone(),
            task: spec.task,
            model: self.model.clone(),
            policy: spec.policy,
            status: AgentStatus::Queued,
        };
        self.agents
            .write()
            .await
            .insert(id.clone(), AgentEntry { info, control });
        self.sub_index.write().await.insert(sub_id, id.clone());

        self.runtime
            .publish(&topic, task)
            .await
            .wrap_err("failed to publish task")?;

        Ok(id)
    }

    /// Cooperatively cancel a single agent (takes effect at the next hook).
    pub async fn cancel(&self, id: &str) -> bool {
        if let Some(entry) = self.agents.write().await.get_mut(id) {
            entry.control.cancel();
            entry.info.status = AgentStatus::Cancelled;
            true
        } else {
            false
        }
    }

    /// Cancel every agent. The environment/runtime stays alive for new spawns.
    pub async fn cancel_all(&self) {
        let mut agents = self.agents.write().await;
        for entry in agents.values_mut() {
            entry.control.cancel();
            if matches!(entry.info.status, AgentStatus::Queued | AgentStatus::Running) {
                entry.info.status = AgentStatus::Cancelled;
            }
        }
    }

    /// Reads AutoAgents protocol events, updates agent status, and re-broadcasts
    /// each (tagged with the owning agent id) to dashboard clients.
    fn spawn_event_pump(self: Arc<Self>, mut stream: autoagents::core::utils::BoxEventStream<Event>) {
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

                // Don't resurrect a cancelled agent.
                if let Some(status) = event_status(&event)
                    && let Some(entry) = self.agents.write().await.get_mut(&agent_id)
                    && entry.info.status != AgentStatus::Cancelled
                {
                    entry.info.status = status;
                }

                if let Ok(value) = serde_json::to_value(&event) {
                    // Ignore send errors: no subscribers is fine.
                    let _ = self.events_tx.send(SwarmEvent {
                        agent_id,
                        event: value,
                    });
                }
            }
        });
    }
}

/// Extract the submission id from any event that carries one.
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

/// Map terminal/transition events to an agent status update.
fn event_status(event: &Event) -> Option<AgentStatus> {
    match event {
        Event::TaskStarted { .. } => Some(AgentStatus::Running),
        Event::TaskComplete { .. } => Some(AgentStatus::Done),
        Event::TaskError { .. } => Some(AgentStatus::Error),
        _ => None,
    }
}
