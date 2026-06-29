//! Leptos dashboard: an Agents view (spawn/monitor/cancel) and a TO DO view
//! (review/edit/accept/reject the task queue the pipeline produces).

use std::collections::HashMap;

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::Value;

use crate::api::{self, AgentInfo, SwarmEvent, Task};

#[derive(Clone, Debug, PartialEq)]
pub struct LogLine {
    pub class: String,
    pub text: String,
}

#[derive(Clone, Copy, PartialEq)]
enum ViewTab {
    Agents,
    Tasks,
}

#[derive(Clone, Copy, PartialEq)]
enum TaskFilter {
    Active,
    History,
}

#[derive(Clone, Copy)]
struct State {
    agents: RwSignal<Vec<AgentInfo>>,
    logs: RwSignal<HashMap<String, Vec<LogLine>>>,
    selected: RwSignal<Option<String>>,
    model: RwSignal<String>,
    tab: RwSignal<ViewTab>,
    tasks: RwSignal<Vec<Task>>,
    history: RwSignal<Vec<Task>>,
    filter: RwSignal<TaskFilter>,
    task_selected: RwSignal<Option<String>>,
    raw_mode: RwSignal<bool>,
    edit_title: RwSignal<String>,
    edit_body: RwSignal<String>,
}

impl State {
    fn refresh_tasks(self) {
        spawn_local(async move {
            self.tasks.set(api::fetch_tasks().await);
            self.history.set(api::fetch_history().await);
        });
    }

    /// All tasks across active + history (for resolving the selected one).
    fn all_tasks(self) -> Vec<Task> {
        let mut v = self.tasks.get();
        v.extend(self.history.get());
        v
    }

    /// The active task to select after `removed` leaves the active list:
    /// the next one in the list, else the previous, else None.
    fn next_active_after(self, removed: &str) -> Option<Task> {
        let tasks = self.tasks.get();
        let idx = tasks.iter().position(|t| t.id == removed)?;
        tasks
            .get(idx + 1)
            .or_else(|| idx.checked_sub(1).and_then(|i| tasks.get(i)))
            .cloned()
    }

    /// Point the selection + editor at `t`, or clear both when `None`.
    fn select_task(self, t: Option<Task>) {
        match t {
            Some(t) => {
                self.task_selected.set(Some(t.id));
                self.edit_title.set(t.title);
                self.edit_body.set(t.body);
                self.raw_mode.set(false);
            }
            None => {
                self.task_selected.set(None);
                self.edit_title.set(String::new());
                self.edit_body.set(String::new());
            }
        }
    }
}

#[component]
pub fn App() -> impl IntoView {
    let state = State {
        agents: RwSignal::new(Vec::new()),
        logs: RwSignal::new(HashMap::new()),
        selected: RwSignal::new(None),
        model: RwSignal::new("…".to_string()),
        tab: RwSignal::new(ViewTab::Tasks),
        tasks: RwSignal::new(Vec::new()),
        history: RwSignal::new(Vec::new()),
        filter: RwSignal::new(TaskFilter::Active),
        task_selected: RwSignal::new(None),
        raw_mode: RwSignal::new(false),
        edit_title: RwSignal::new(String::new()),
        edit_body: RwSignal::new(String::new()),
    };

    spawn_local(async move {
        state.model.set(api::fetch_model().await);
        state.tasks.set(api::fetch_tasks().await);
        state.history.set(api::fetch_history().await);
        loop {
            state.agents.set(api::fetch_agents().await);
            gloo_timers::future::TimeoutFuture::new(3000).await;
        }
    });

    api::connect_ws(move |ev: SwarmEvent| handle_event(state, ev));

    view! {
        <header>
            <h1>"axon swarm"</h1>
            <nav>
                <button class:active=move || state.tab.get() == ViewTab::Tasks
                    on:click=move |_| { state.tab.set(ViewTab::Tasks); state.refresh_tasks(); }>
                    "TO DO"</button>
                <button class:active=move || state.tab.get() == ViewTab::Agents
                    on:click=move |_| state.tab.set(ViewTab::Agents)>"Agents"</button>
            </nav>
            <span class="model">"model: " {move || state.model.get()}</span>
            <span class="spacer"></span>
            <span id="conn" class="model">"● live"</span>
        </header>
        {move || match state.tab.get() {
            ViewTab::Agents => agents_view(state).into_any(),
            ViewTab::Tasks => tasks_view(state).into_any(),
        }}
    }
}

// --------------------------------------------------------------------------
// Agents view
// --------------------------------------------------------------------------

fn agents_view(state: State) -> impl IntoView {
    view! {
        <main>
            <div class="left">
                <SpawnForm state=state/>
                <div class="row" style="margin:14px 0 8px">
                    <h3 class="section" style="margin:0">"AGENTS"</h3>
                    <span class="spacer"></span>
                    <button class="danger" on:click=move |_| spawn_local(async { api::cancel_all().await; })>
                        "cancel all"</button>
                </div>
                <div class="agents">
                    {move || {
                        let ags = state.agents.get();
                        if ags.is_empty() {
                            view! { <div class="empty">"none yet"</div> }.into_any()
                        } else {
                            ags.into_iter().map(|a| agent_card(state, a)).collect_view().into_any()
                        }
                    }}
                </div>
            </div>
            <div class="right"><Detail state=state/></div>
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
            <textarea prop:value=move || task.get()
                on:input=move |e| task.set(event_target_value(&e))
                placeholder="Describe a task for a new agent…"></textarea>
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
            let status = state.agents.get().into_iter().find(|a| a.id == id)
                .map(|a| a.status).unwrap_or_default();
            view! {
                <h3>{id.clone()} " — " {status}</h3>
                <div class="log">
                    {lines.into_iter()
                        .map(|l| view! { <div class=format!("line {}", l.class)>{l.text}</div> })
                        .collect_view()}
                </div>
            }.into_any()
        }
    }
}

fn agent_card(state: State, a: AgentInfo) -> impl IntoView {
    let id_sel = a.id.clone();
    let id_cancel = a.id.clone();
    let selected = move || state.selected.get().as_deref() == Some(id_sel.as_str());
    let system = a.perpetual;
    let role = a.role.clone();
    view! {
        <div class=move || if selected() { "agent sel" } else { "agent" }
            on:click={ let id = a.id.clone(); move |_| state.selected.set(Some(id.clone())) }>
            <div class="row">
                <span class="id">{a.id.clone()}</span>
                {move || system.then(|| view! { <span class="badge sys">{role.clone()}</span> })}
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

// --------------------------------------------------------------------------
// TO DO (tasks) view
// --------------------------------------------------------------------------

fn tasks_view(state: State) -> impl IntoView {
    view! {
        <main>
            <div class="left">
                <div class="row" style="margin-bottom:8px">
                    <h3 class="section" style="margin:0">"TASKS"</h3>
                    <span class="spacer"></span>
                    <button class:active=move || state.filter.get() == TaskFilter::Active
                        on:click=move |_| state.filter.set(TaskFilter::Active)>"Active"</button>
                    <button class:active=move || state.filter.get() == TaskFilter::History
                        on:click=move |_| state.filter.set(TaskFilter::History)>"History"</button>
                </div>
                <div class="agents">
                    {move || {
                        let items = match state.filter.get() {
                            TaskFilter::Active => state.tasks.get(),
                            TaskFilter::History => state.history.get(),
                        };
                        if items.is_empty() {
                            view! { <div class="empty">"nothing here yet"</div> }.into_any()
                        } else {
                            items.into_iter().map(|t| task_card(state, t)).collect_view().into_any()
                        }
                    }}
                </div>
            </div>
            <div class="right"><TaskDetail state=state/></div>
        </main>
    }
}

fn task_card(state: State, t: Task) -> impl IntoView {
    let id = t.id.clone();
    let selected = { let id = id.clone(); move || state.task_selected.get().as_deref() == Some(id.as_str()) };
    let open = {
        let t = t.clone();
        move |_| {
            state.task_selected.set(Some(t.id.clone()));
            state.edit_title.set(t.title.clone());
            state.edit_body.set(t.body.clone());
            state.raw_mode.set(false);
        }
    };
    view! {
        <div class=move || if selected() { "agent sel" } else { "agent" } on:click=open>
            <div class="row">
                <span class="id">{t.title.clone()}</span>
                <span class="spacer"></span>
                <span class=format!("badge {}", t.status)>{t.status.clone()}</span>
            </div>
            <div class="task">{t.description.clone()}</div>
        </div>
    }
}

#[component]
fn TaskDetail(state: State) -> impl IntoView {
    move || match state.task_selected.get() {
        None => view! { <h3 class="muted">"select a task to review"</h3> }.into_any(),
        Some(id) => {
            let task = state.all_tasks().into_iter().find(|t| t.id == id);
            let actionable = task.as_ref().map(|t| {
                matches!(t.status.as_str(), "proposed" | "accepted")
            }).unwrap_or(false);
            let status = task.map(|t| t.status).unwrap_or_default();
            let id_save = id.clone();
            let id_accept = id.clone();
            let id_reject = id.clone();
            view! {
                <div class="row" style="margin-bottom:8px">
                    <h3 style="margin:0">{move || state.edit_title.get()}</h3>
                    <span class=format!("badge {status}")>{status.clone()}</span>
                    <span class="spacer"></span>
                    <button on:click=move |_| state.raw_mode.update(|r| *r = !*r)>
                        {move || if state.raw_mode.get() { "view" } else { "edit" }}</button>
                    {actionable.then(|| view! {
                        <button on:click=move |_| {
                            let id = id_accept.clone();
                            spawn_local(async move { api::accept_task(&id).await; state.refresh_tasks(); });
                        }>"Accept"</button>
                        <button class="danger" on:click=move |_| {
                            let id = id_reject.clone();
                            let next = state.next_active_after(&id);
                            spawn_local(async move {
                                api::reject_task(&id).await;
                                state.refresh_tasks();
                                state.select_task(next);
                            });
                        }>"Reject"</button>
                    })}
                </div>
                {move || if state.raw_mode.get() {
                    let id_save = id_save.clone();
                    view! {
                        <div class="field">
                            <input class="title-edit" prop:value=move || state.edit_title.get()
                                on:input=move |e| state.edit_title.set(event_target_value(&e)) />
                        </div>
                        <textarea class="md-edit" prop:value=move || state.edit_body.get()
                            on:input=move |e| state.edit_body.set(event_target_value(&e))></textarea>
                        <button style="margin-top:8px" on:click=move |_| {
                            let id = id_save.clone();
                            spawn_local(async move {
                                let _ = api::update_task(&id, &state.edit_title.get_untracked(),
                                    &state.edit_body.get_untracked()).await;
                                state.refresh_tasks();
                            });
                        }>"Save"</button>
                    }.into_any()
                } else {
                    view! {
                        <div class="md-preview" inner_html=move || md_to_html(&state.edit_body.get())></div>
                    }.into_any()
                }}
            }.into_any()
        }
    }
}

fn md_to_html(src: &str) -> String {
    let parser = pulldown_cmark::Parser::new(src);
    let mut out = String::new();
    pulldown_cmark::html::push_html(&mut out, parser);
    out
}

// --------------------------------------------------------------------------
// Events
// --------------------------------------------------------------------------

fn handle_event(state: State, ev: SwarmEvent) {
    let id = ev.agent_id;
    let Some((kind, data)) = ev.event.as_object().and_then(|o| o.iter().next()) else {
        return;
    };

    if id == "system" {
        match kind.as_str() {
            "Reload" => {
                if let Some(w) = web_sys::window() {
                    let _ = w.location().reload();
                }
            }
            "TasksChanged" => state.refresh_tasks(),
            _ => {}
        }
        return;
    }

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
        state.logs.update(|m| m.entry(id.clone()).or_default().push(line));
    }
}

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
            let t = d.get("chunk").and_then(|c| c.get("delta").or_else(|| c.get("response")))
                .and_then(|v| v.as_str()).unwrap_or("");
            if t.is_empty() {
                return None;
            }
            ("", t.to_string())
        }
        "TaskComplete" => ("ev-done", format!("✅ {}", trunc(&s("result"), 4000))),
        "TaskError" => ("ev-error", format!("❌ {}", s("error"))),
        _ => return None,
    };
    Some(LogLine { class: class.to_string(), text })
}

fn trunc(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        format!("{}…", s.chars().take(n).collect::<String>())
    } else {
        s.to_string()
    }
}
