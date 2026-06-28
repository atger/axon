//! Leptos dashboard: spawn agents, list them with live status, and stream each
//! agent's trace over the `/ws` WebSocket.

use std::collections::HashMap;

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::Value;

use crate::api::{self, AgentInfo, SwarmEvent};

/// A single rendered trace line with a CSS class for colouring.
#[derive(Clone, Debug, PartialEq)]
pub struct LogLine {
    pub class: String,
    pub text: String,
}

/// Reactive app state. `RwSignal` is `Copy`, so this whole struct is `Copy` and
/// can be captured freely in closures / passed via context.
#[derive(Clone, Copy)]
struct State {
    agents: RwSignal<Vec<AgentInfo>>,
    logs: RwSignal<HashMap<String, Vec<LogLine>>>,
    selected: RwSignal<Option<String>>,
    model: RwSignal<String>,
}

#[component]
pub fn App() -> impl IntoView {
    let state = State {
        agents: RwSignal::new(Vec::new()),
        logs: RwSignal::new(HashMap::new()),
        selected: RwSignal::new(None),
        model: RwSignal::new("…".to_string()),
    };

    // Initial load + poll loop for authoritative agent snapshots.
    spawn_local(async move {
        state.model.set(api::fetch_model().await);
        loop {
            state.agents.set(api::fetch_agents().await);
            gloo_timers::future::TimeoutFuture::new(3000).await;
        }
    });

    // Live event stream: update status + append trace lines.
    api::connect_ws(move |ev: SwarmEvent| handle_event(state, ev));

    view! {
        <header>
            <h1>"axon swarm"</h1>
            <span class="model">"model: " {move || state.model.get()}</span>
            <span class="spacer"></span>
            <button class="danger" on:click=move |_| spawn_local(async { api::cancel_all().await; })>
                "cancel all"
            </button>
        </header>
        <main>
            <div class="left">
                <SpawnForm state=state/>
                <h3 class="section">"AGENTS"</h3>
                <div class="agents">
                    {move || {
                        let ags = state.agents.get();
                        if ags.is_empty() {
                            view! { <div class="empty">"none yet"</div> }.into_any()
                        } else {
                            ags.into_iter()
                                .map(|a| agent_card(state, a))
                                .collect_view()
                                .into_any()
                        }
                    }}
                </div>
            </div>
            <div class="right">
                <Detail state=state/>
            </div>
        </main>
    }
}

#[component]
fn SpawnForm(state: State) -> impl IntoView {
    let task = RwSignal::new(String::new());
    let policy = RwSignal::new("auto_approve".to_string());

    let submit = move |_| {
        let t = task.get().trim().to_string();
        if t.is_empty() {
            return;
        }
        let p = policy.get();
        task.set(String::new());
        spawn_local(async move {
            let _ = api::spawn_agent(t, p).await;
            state.agents.set(api::fetch_agents().await);
        });
    };

    view! {
        <div class="field">
            <label>"task"</label>
            <textarea
                prop:value=move || task.get()
                on:input=move |e| task.set(event_target_value(&e))
                placeholder="Describe a task for a new agent…"
            ></textarea>
        </div>
        <div class="field">
            <label>"approval policy"</label>
            <select on:change=move |e| policy.set(event_target_value(&e))>
                <option value="auto_approve">"auto-approve (autonomous)"</option>
                <option value="deny_destructive">"deny destructive (read-only safe)"</option>
            </select>
        </div>
        <button on:click=submit>"spawn agent"</button>
    }
}

#[component]
fn Detail(state: State) -> impl IntoView {
    move || match state.selected.get() {
        None => view! { <h3 class="muted">"select an agent to view its trace"</h3> }.into_any(),
        Some(id) => {
            let lines = state.logs.get().get(&id).cloned().unwrap_or_default();
            let status = state
                .agents
                .get()
                .into_iter()
                .find(|a| a.id == id)
                .map(|a| a.status)
                .unwrap_or_default();
            view! {
                <h3>{id.clone()} " — " {status}</h3>
                <div class="log">
                    {lines
                        .into_iter()
                        .map(|l| view! { <div class=format!("line {}", l.class)>{l.text}</div> })
                        .collect_view()}
                </div>
            }
            .into_any()
        }
    }
}

/// Render one agent card (plain fn so it can capture `state` by copy).
fn agent_card(state: State, a: AgentInfo) -> impl IntoView {
    let id_sel = a.id.clone();
    let id_cancel = a.id.clone();
    let selected = move || state.selected.get().as_deref() == Some(id_sel.as_str());
    view! {
        <div
            class=move || if selected() { "agent sel" } else { "agent" }
            on:click={
                let id = a.id.clone();
                move |_| state.selected.set(Some(id.clone()))
            }
        >
            <div class="row">
                <span class="id">{a.id.clone()}</span>
                <span class=format!("badge {}", a.status)>{a.status.clone()}</span>
                <span class="spacer"></span>
                <button on:click=move |e| {
                    e.stop_propagation();
                    let id = id_cancel.clone();
                    spawn_local(async move { api::cancel_agent(id).await; });
                }>"cancel"</button>
            </div>
            <div class="task">{a.task.clone()}</div>
        </div>
    }
}

/// Apply a swarm event to reactive state: update status + append a trace line.
fn handle_event(state: State, ev: SwarmEvent) {
    let id = ev.agent_id;
    let Some((kind, data)) = ev.event.as_object().and_then(|o| o.iter().next()) else {
        return;
    };

    if let Some(status) = match kind.as_str() {
        "TaskStarted" => Some("running"),
        "TaskComplete" => Some("done"),
        "TaskError" => Some("error"),
        _ => None,
    } {
        state.agents.update(|ags| {
            if let Some(a) = ags.iter_mut().find(|a| a.id == id) {
                a.status = status.to_string();
            }
        });
    }

    if let Some(line) = format_event(kind, data) {
        state
            .logs
            .update(|m| m.entry(id.clone()).or_default().push(line));
    }
}

/// Turn an AutoAgents protocol event into a displayable trace line.
fn format_event(kind: &str, d: &Value) -> Option<LogLine> {
    let s = |k: &str| d.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let (class, text) = match kind {
        "TaskStarted" => ("ev-turn", format!("▶ task started: {}", s("task_description"))),
        "TurnStarted" => (
            "ev-turn",
            format!(
                "— turn {}/{}",
                d.get("turn_number").and_then(|v| v.as_u64()).unwrap_or(0) + 1,
                d.get("max_turns").and_then(|v| v.as_u64()).unwrap_or(0)
            ),
        ),
        "ToolCallRequested" => ("ev-tool", format!("🔧 {}({})", s("tool_name"), trunc(&s("arguments"), 200))),
        "ToolCallCompleted" => (
            "ev-tool",
            format!("✓ {} → {}", s("tool_name"), trunc(&d.get("result").map(|v| v.to_string()).unwrap_or_default(), 200)),
        ),
        "ToolCallFailed" => ("ev-error", format!("✗ {}: {}", s("tool_name"), s("error"))),
        "StreamChunk" => {
            let t = d
                .get("chunk")
                .and_then(|c| c.get("delta").or_else(|| c.get("response")))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if t.is_empty() {
                return None;
            }
            ("", t.to_string())
        }
        "TaskComplete" => ("ev-done", format!("✅ {}", trunc(&s("result"), 4000))),
        "TaskError" => ("ev-error", format!("❌ {}", s("error"))),
        _ => return None,
    };
    Some(LogLine {
        class: class.to_string(),
        text,
    })
}

fn trunc(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        format!("{}…", s.chars().take(n).collect::<String>())
    } else {
        s.to_string()
    }
}
