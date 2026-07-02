//! Leptos dashboard: an Agents view (spawn/monitor/cancel) and a Tasks view
//! (review/edit/accept/reject the task queue the pipeline produces).

use std::collections::HashMap;

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::Value;

use crate::api::{self, AgentDef, AgentInfo, SwarmEvent, Task, TeamWithAgents, McpTool};

#[derive(Clone, Debug, PartialEq)]
pub struct LogLine {
    pub class: String,
    pub text: String,
}

#[derive(Clone, Copy, PartialEq)]
enum ViewTab {
    Agents,
    Tasks,
    Settings,
}

#[derive(Clone, Copy, PartialEq)]
enum SettingsOption {
    MCP,
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
    teams: RwSignal<Vec<TeamWithAgents>>,
    models: RwSignal<Vec<String>>,
    editing_def: RwSignal<bool>,
    ed_md: RwSignal<String>,
    ed_def: RwSignal<Option<AgentDef>>,
    raw_mode_def: RwSignal<bool>,
    spawn_task: RwSignal<String>,
    mcp_servers: RwSignal<HashMap<String, api::McpServerConfig>>,
    editing_mcp: RwSignal<bool>,
    mcp_json_buffer: RwSignal<String>,
    settings_opt: RwSignal<SettingsOption>,
    /// Visibility of the tools sidebar in the spawn window
    tools_sidebar_visible: RwSignal<bool>,
    /// Available MCP tools from connected servers
    mcp_tools: RwSignal<Vec<McpTool>>,
    /// Completed cycles saved per agent so they remain visible even after
    /// cycle_started is updated on the next publish.
    prev_cycles: RwSignal<HashMap<String, Vec<(String, String, String)>>>,
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
        teams: RwSignal::new(Vec::new()),
        models: RwSignal::new(Vec::new()),
        editing_def: RwSignal::new(false),
        ed_md: RwSignal::new(String::new()),
        ed_def: RwSignal::new(None),
        raw_mode_def: RwSignal::new(false),
        spawn_task: RwSignal::new(String::new()),
        mcp_servers: RwSignal::new(HashMap::new()),
        editing_mcp: RwSignal::new(false),
        mcp_json_buffer: RwSignal::new(String::new()),
        settings_opt: RwSignal::new(SettingsOption::MCP),
        tools_sidebar_visible: RwSignal::new(false),
        mcp_tools: RwSignal::new(Vec::new()),
        prev_cycles: RwSignal::new(HashMap::new()),
    };

    spawn_local(async move {
        state.model.set(api::fetch_model().await);
        state.tasks.set(api::fetch_tasks().await);
        state.history.set(api::fetch_history().await);
        state.mcp_servers.set(api::fetch_mcp().await);
        let teams = api::fetch_teams().await;
        if !teams.iter().any(|t| t.team.name == "custom") {
            let _ = api::create_team("custom").await;
            state.teams.set(api::fetch_teams().await);
        } else {
            state.teams.set(teams);
        }
        state.models.set(api::fetch_models().await);
        state.mcp_tools.set(api::fetch_mcp_tools().await);
        loop {
            state.agents.set(api::fetch_agents().await);
            state.mcp_tools.set(api::fetch_mcp_tools().await);
            gloo_timers::future::TimeoutFuture::new(3000).await;
        }
    });

    api::connect_ws(move |ev: SwarmEvent| handle_event(state, ev));

    view! {
        <div class="app-shell">
            <header>
                <h1>"axon swarm"</h1>
                <nav>
                    <button class:active=move || state.tab.get() == ViewTab::Tasks
                        on:click=move |_| { state.tab.set(ViewTab::Tasks); state.refresh_tasks(); }>
                        "Tasks"</button>
                    <button class:active=move || state.tab.get() == ViewTab::Agents
                        on:click=move |_| { state.tab.set(ViewTab::Agents); state.editing_def.set(false); state.selected.set(None); }>"Agents"</button>
                </nav>
                <span class="model-label" style="font-size: 13px; color: var(--muted); background: var(--btn-bg); padding: 4px 10px; border-radius: 4px; border: 1px solid var(--border);">
                    "Model: " {move || state.model.get()}
                </span>
                <span class="spacer"></span>
                <button class="settings-btn" class:active=move || state.tab.get() == ViewTab::Settings
                    on:click=move |_| { state.tab.set(ViewTab::Settings); } title="Settings">
                    <span style="font-size: 13px; margin-right: 6px; font-weight: 500;">"Settings"</span>
                    "⚙"
                </button>
            </header>
            {move || match state.tab.get() {
                ViewTab::Agents => agents_view(state).into_any(),
                ViewTab::Tasks => tasks_view(state).into_any(),
                ViewTab::Settings => settings_view(state).into_any(),
            }}
        </div>
    }
}

// --------------------------------------------------------------------------
// Agents view
// --------------------------------------------------------------------------

fn agents_view(state: State) -> impl IntoView {
    let spawn_input: NodeRef<leptos::html::Textarea> = NodeRef::new();

    let open_def = move |d: AgentDef| {
        leptos::logging::log!("open_def called for: {}", d.name);
        state.ed_md.set(agent_def_to_md(&d));
        state.ed_def.set(Some(d));
        state.raw_mode_def.set(false);
        state.spawn_task.set(String::new());
        state.editing_def.set(true);
        state.selected.set(None);
        leptos::logging::log!("editing_def now: {}", state.editing_def.get());
    };

    let save = move || {
        let def = state
            .ed_def
            .get()
            .unwrap_or_else(|| blank_def("custom".to_string()));
        let updated = md_to_agent_def(&state.ed_md.get(), &def);
        let id = if updated.id.is_empty() {
            None
        } else {
            Some(updated.id.clone())
        };
        let team = updated.team_id.clone();
        state.raw_mode_def.set(false);
        spawn_local(async move {
            let _ = match id {
                Some(id) => api::update_def(&id, &updated).await,
                None => api::create_def(&team, &updated).await,
            };
            state.teams.set(api::fetch_teams().await);
            if state.editing_def.get() {
                state.ed_def.set(Some(updated));
            } else {
                state.editing_def.set(false);
            }
        });
    };

    let del = move |_| {
        let id = state.ed_def.get().and_then(|d| {
            if d.id.is_empty() {
                None
            } else {
                Some(d.id.clone())
            }
        });
        if let Some(id) = id {
            state.editing_def.set(false);
            spawn_local(async move {
                let _ = api::delete_def(&id).await;
                state.teams.set(api::fetch_teams().await);
            });
        }
    };

    let do_spawn = move |_| {
        let task = state.spawn_task.get().trim().to_string();
        if task.is_empty() {
            return;
        }
        if let Some(def) = state.ed_def.get() {
            if !def.id.is_empty() {
                let id = def.id.clone();
                if let Some(el) = spawn_input.get() {
                    let _ = el.set_value("");
                }
                state.editing_def.set(false);
                spawn_local(async move {
                    let _ = api::spawn_agent(id, task).await;
                    state.agents.set(api::fetch_agents().await);
                });
            }
        }
    };

    view! {
        <main>
            <div class="left">
                <div class="row toolbar">
                    <h3 class="section">"AGENTS"</h3>
                    <span class="spacer"></span>
                    <button class="danger"
                        on:click=move |_| spawn_local(async { api::cancel_all().await; })>
                        "cancel all"</button>
                </div>
                <div class="agents">
                    {move || {
                        let teams = state.teams.get();
                        let mut items = Vec::new();
                        for tw in &teams {
                            for a in &tw.agents {
                                items.push((tw.team.name.clone(), a.clone()));
                            }
                        }
                        if items.is_empty() {
                            view! { <div class="empty">"no definitions yet"</div> }.into_any()
                        } else {
                            items.into_iter().map(|(team_name, a)| {
                                let aid = a.id.clone();
                                let is_sel = move || state.editing_def.get()
                                    && state.ed_def.get().as_ref().map(|d| d.id == aid).unwrap_or(false);
                                let ac = a.clone();
                                let a_clone = a.clone();
                                let tn = team_name.clone();
                                let active_count = {
                                    let name = a.name.clone();
                                    move || state.agents.get().iter().filter(|ag| ag.def_name.as_deref() == Some(&name)).count()
                                };
                                view! {
                                    <div class=move || if is_sel() { "agent sel" } else { "agent" }
                                        on:click=move |_| open_def(a_clone.clone())>
                                        <div class="row">
                                            <span class="badge sys">{tn.clone()}</span>
                                            <span class="id">{ac.name.clone()}</span>
                                        </div>
                                        <div class="task">{ac.instructions.clone()}</div>
                                    </div>
                                }
                            }).collect_view().into_any()
                        }
                    }}
                </div>
            </div>
            <div class="right">
                {move || {
                    leptos::logging::log!("right panel: selected={:?}, editing_def={}, raw_mode_def={}, ed_def={:?}",
                        state.selected.get(), state.editing_def.get(), state.raw_mode_def.get(),
                        state.ed_def.get().map(|d| d.name));
                    if state.selected.get().is_some() {
                        view! {
                            <div class="split">
                                <div class="split-top">
                                    <TimelineView state=state />
                                </div>
                                <div class="split-bottom">
                                    <Detail state=state/>
                                </div>
                            </div>
                        }.into_any()
                    } else if state.editing_def.get() {
                        let ro = state.ed_def.get().map(|d| d.builtin).unwrap_or(false);
                        let name = state.ed_def.get().as_ref().map(|d| d.name.as_str()).unwrap_or("agent").to_string();
                        if state.raw_mode_def.get() {
                            view! {
                                <div class="row toolbar">
                                    <h3>{name.clone()}</h3>
                                    <span class="spacer"></span>
                                    {ro.then(|| view! { <span class="badge sys">"read-only"</span> })}
                                    <button on:click=move |_| state.raw_mode_def.set(false)>"Back"</button>
                                </div>
                                <textarea class="md-edit" prop:value=move || state.ed_md.get()
                                    on:input=move |e| state.ed_md.set(event_target_value(&e))></textarea>
                                <div class="row mt-sm">
                                    <button on:click=move |_| { save(); }>"Save"</button>
                                    {state.ed_def.get().map(|d| !d.id.is_empty() && !d.builtin).unwrap_or(false).then(|| view! {
                                        <button class="danger" on:click=del>"Delete"</button>
                                    })}
                                    <button on:click=move |_| { state.raw_mode_def.set(false); }>"Cancel"</button>
                                </div>
                            }.into_any()
                        } else {
                            view! {
                                <div class="row toolbar">
                                    <select
                                        on:change=move |ev| {
                                            let val = event_target_value(&ev);
                                            let model_opt = if val == "default" { None } else { Some(val) };
                                            if let Some(mut def) = state.ed_def.get() {
                                                let id = def.id.clone();
                                                if !id.is_empty() {
                                                    def.model = model_opt;
                                                    let def_clone = def.clone();
                                                    state.ed_def.set(Some(def_clone.clone()));
                                                    state.ed_md.set(agent_def_to_md(&def_clone));
                                                    spawn_local(async move {
                                                        let _ = api::update_def(&id, &def_clone).await;
                                                        state.teams.set(api::fetch_teams().await);
                                                    });
                                                }
                                            }
                                        }
                                        prop:value=move || state.ed_def.get().and_then(|d| d.model).unwrap_or_else(|| "default".to_string())
                                        style="width: auto; max-width: 180px;">
                                        <option value="default">"Default Model"</option>
                                        {move || state.models.get().into_iter().map(|m| {
                                            view! { <option value=m.clone()>{m.clone()}</option> }
                                        }).collect_view()}
                                    </select>
                                    <span class="spacer"></span>
                                    <button on:click=move |_| state.raw_mode_def.set(true)>"Edit"</button>
                                    <button on:click=move |_| state.tools_sidebar_visible.update(|v| *v = !*v)
                                        class:active=move || state.tools_sidebar_visible.get()>
                                        "Tools"
                                    </button>
                                </div>
                                <div class="spawn-center">
                                    <h2 style="margin-bottom: 24px; color: var(--accent);">{name.clone()}</h2>
                                    <div class="spawn-section" style="border:none; margin-top:0; padding-top:0; width: 100%; max-width: 500px;">
                                        <div class="field">
                                            <textarea node_ref=spawn_input
                                                on:input=move |e| state.spawn_task.set(event_target_value(&e))
                                                placeholder=move || state.ed_def.get().and_then(|d| d.task_hint.clone()).unwrap_or_else(|| "Describe a task for this agent…".to_string())
                                                style="min-height: 120px;"></textarea>
                                            <div class="row mt-sm" style="gap: 8px;">
                                                <select
                                                    on:change=move |ev| {
                                                        let val = event_target_value(&ev);
                                                        let mins = val.parse::<u64>().unwrap_or(0);
                                                        if let Some(mut def) = state.ed_def.get() {
                                                            let id = def.id.clone();
                                                            if !id.is_empty() {
                                                                def.schedule_mins = if mins > 0 { Some(mins) } else { None };
                                                                let current_task = state.spawn_task.get();
                                                                if !current_task.is_empty() {
                                                                    def.task = Some(current_task);
                                                                }
                                                                let def_clone = def.clone();
                                                                state.ed_def.set(Some(def_clone.clone()));
                                                                state.ed_md.set(agent_def_to_md(&def_clone));
                                                                spawn_local(async move {
                                                                    let _ = api::update_def(&id, &def_clone).await;
                                                                    state.teams.set(api::fetch_teams().await);
                                                                });
                                                            }
                                                        }
                                                    }
                                                    prop:value=move || state.ed_def.get().and_then(|d| d.schedule_mins).unwrap_or(0).to_string()
                                                    style="width: auto; height: 40px;">
                                                    <option value="0">"Manual"</option>
                                                    <option value="1">"1m"</option>
                                                    <option value="5">"5m"</option>
                                                    <option value="15">"15m"</option>
                                                    <option value="30">"30m"</option>
                                                    <option value="60">"1h"</option>
                                                    <option value="360">"6h"</option>
                                                    <option value="720">"12h"</option>
                                                    <option value="1440">"24h"</option>
                                                </select>
                                                <button on:click=do_spawn style="flex: 1; height: 40px; font-weight: bold;">"Spawn"</button>
                                            </div>
                                        </div>
                                    </div>
                                </div>
                            }.into_any()
                        }
                    } else {
                        view! { <TimelineView state=state /> }.into_any()
                    }
                }}
            </div>
        </main>
        {move || state.tools_sidebar_visible.get().then(|| view! { <ToolsSidebar state=state /> })}
    }
}

#[component]
fn TimelineView(state: State) -> impl IntoView {
    const LX: f64 = 5.0;
    const RH: f64 = 48.0;
    const HH: f64 = 40.0;
    const ZOOM_LEVELS: &[f64] = &[1200.0, 2400.0, 3600.0, 7200.0, 14400.0, 28800.0, 86400.0];

    let zoom_idx = RwSignal::new(1i32);
    let wrap_ref: NodeRef<leptos::html::Div> = NodeRef::new();
    let svg_w = RwSignal::new(900.0f64);

    Effect::new(move |_| {
        if let Some(el) = wrap_ref.get() {
            let w = (el.client_width() as f64).max(600.0);
            svg_w.set(w);
        }
    });

    let rows = move || -> Vec<(String, u64, Vec<api::AgentInfo>)> {
        let agents = state.agents.get();
        let teams = state.teams.get();
        let mut def_map: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
        for tw in &teams {
            for def in &tw.agents {
                if let Some(mins) = def.schedule_mins {
                    if mins > 0 {
                        def_map.insert(def.name.clone(), mins);
                    }
                }
            }
        }
        let mut groups: Vec<(String, u64, Vec<api::AgentInfo>)> = Vec::new();
        for agent in agents.iter() {
            if let Some(def_name) = &agent.def_name {
                let mins = def_map.get(def_name.as_str()).cloned().unwrap_or(0);
                let short = if agent.id.len() > 6 {
                    &agent.id[..6]
                } else {
                    &agent.id
                };
                groups.push((
                    format!("{} ({})", def_name, short),
                    mins,
                    vec![agent.clone()],
                ));
            }
        }
        groups.sort_by(|a, b| {
            let a_def = a.0.split(" (").next().unwrap_or(&a.0);
            let b_def = b.0.split(" (").next().unwrap_or(&b.0);
            a_def.cmp(&b_def).then_with(|| a.0.cmp(&b.0))
        });
        groups
    };

    let nrows = move || rows().len();

    let nonscheduled = move || {
        let agents = state.agents.get();
        let teams = state.teams.get();
        let scheduled_names: std::collections::HashSet<String> = teams
            .iter()
            .flat_map(|tw| &tw.agents)
            .filter(|d| d.schedule_mins.unwrap_or(0) > 0)
            .map(|d| d.name.clone())
            .collect();
        agents
            .iter()
            .filter(|a| {
                a.def_name
                    .as_deref()
                    .map(|n| !scheduled_names.contains(n))
                    .unwrap_or(true)
            })
            .count()
    };

    let view_box = move || {
        let n = nrows();
        let w = svg_w.get();
        if n == 0 {
            format!("0 0 {w} 200")
        } else {
            format!("0 0 {w} {}", HH + n as f64 * RH + 30.0)
        }
    };

    let zoom_max = ZOOM_LEVELS.len() as i32 - 1;
    let zoom_out = move |_| {
        zoom_idx.update(|i| {
            if *i < zoom_max {
                *i += 1
            }
        })
    };
    let zoom_in = move |_| {
        zoom_idx.update(|i| {
            if *i > 0 {
                *i -= 1
            }
        })
    };

    view! {
        <div class="timeline-wrap" node_ref=wrap_ref>
            <div class="timeline-controls">
                <button on:click=zoom_out>"−"</button>
                <span class="zoom-label">{move || {
                    let t = ZOOM_LEVELS[zoom_idx.get() as usize];
                    if t < 3600.0 { format!("{}m", t / 60.0) }
                    else if t < 86400.0 { format!("{}h", t / 3600.0) }
                    else { format!("24h") }
                }}</span>
                <button on:click=zoom_in>"+"</button>
            </div>
            <svg class="timeline-svg" viewBox=view_box preserveAspectRatio="xMinYMin meet"
                on:click=move |_| state.selected.set(None)>
                {move || {
                    let now = js_sys::Date::now() / 1000.0;
                    let n = nrows();
                    let wt = ZOOM_LEVELS[zoom_idx.get() as usize];
                    let wp = wt * 0.75;
                    let wf = wt * 0.25;
                    let w = svg_w.get();
                    let tx = LX + 135.0;
                    let tw = w - tx - 20.0;
                    if n == 0 {
                        return view! { <text x={(w / 2.0).to_string()} y="100" text-anchor="middle" fill="var(--muted)" font-size="14">"no scheduled agents"</text> }.into_any();
                    }
                    let t2x = move |t_epoch: f64| -> f64 { tx + (t_epoch - (now - wp)) / wt * tw };

                    // ── time axis ──
                    let tick_int = move || -> f64 {
                        if wt <= 1200.0 { 120.0 }
                        else if wt <= 2400.0 { 300.0 }
                        else if wt <= 3600.0 { 600.0 }
                        else if wt <= 7200.0 { 900.0 }
                        else if wt <= 14400.0 { 1800.0 }
                        else if wt <= 28800.0 { 3600.0 }
                        else { 10800.0 }
                    };
                    let ti = tick_int();
                    let tick_count = ((wp + wf) / ti).ceil() as i32;
                    let axis: Vec<_> = (0..=tick_count).filter_map(|i| {
                        let offset_s = i as f64 * ti - wp;
                        let x = t2x(now + offset_s);
                        if x < tx || x > tx + tw { return None; }
                        let label = if offset_s == 0.0 {
                            "now".to_string()
                        } else if wt <= 3600.0 {
                            format!("{:+}m", (offset_s / 60.0) as i32)
                        } else {
                            format!("{:+}h", (offset_s / 3600.0) as i32)
                        };
                        Some(view! {
                            <g>
                                <text x={x.to_string()} y="16" text-anchor="middle" fill="var(--muted)" font-size="10">{label}</text>
                                <line x1={x.to_string()} y1="22" x2={x.to_string()} y2="28" stroke="var(--border)" stroke-width="1" />
                            </g>
                        }.into_any())
                    }).collect();

                    // ── rows ──
                    let row_views: Vec<_> = rows().into_iter().enumerate().map(|(i, (name, interval_mins, ags))| {
                        let y = HH + i as f64 * RH;
                        let y_center = y + RH / 2.0;
                        let label = if name.len() > 18 { format!("{}…", &name[..18]) } else { name.clone() };
                        let interval_s = interval_mins as f64 * 60.0;
                        let primary = ags.first();
                        let status = primary.map(|a| a.status.as_str()).unwrap_or("");
                        let cs = primary.and_then(|a| {
                            let s = a.cycle_started.as_str();
                            if s.is_empty() { return None; }
                            let js_val = wasm_bindgen::JsValue::from_str(s);
                            let d = js_sys::Date::new(&js_val);
                            let t = d.get_time() / 1000.0;
                            if t.is_nan() || t <= 0.0 { None } else { Some(t) }
                        });

                        let agent_started = primary.and_then(|a| {
                            let s = a.started.as_str();
                            if s.is_empty() { return None; }
                            let js_val = wasm_bindgen::JsValue::from_str(s);
                            let d = js_sys::Date::new(&js_val);
                            let t = d.get_time() / 1000.0;
                            if t.is_nan() || t <= 0.0 { None } else { Some(t) }
                        });

                        let cycle_completed = primary.and_then(|a| {
                            let s = a.cycle_completed.as_str();
                            if s.is_empty() { return None; }
                            let js_val = wasm_bindgen::JsValue::from_str(s);
                            let d = js_sys::Date::new(&js_val);
                            let t = d.get_time() / 1000.0;
                            if t.is_nan() || t <= 0.0 { None } else { Some(t) }
                        });

                        // Build cycle bars within the visible window
                        let mut bars = Vec::new();
                        let win_start = now - wp;
                        let win_end = now + wf;

                        let is_manual = interval_mins == 0;

                        if is_manual {
                            // Manual row: render a single bar spanning from its start time to complete (or now)
                            if let Some(started) = agent_started {
                                let bar_end = cycle_completed.unwrap_or(now);
                                let x1 = t2x(started.max(win_start)).max(tx);
                                let x2 = t2x(bar_end.min(win_end)).min(tx + tw);
                                let w = (x2 - x1).max(2.0);
                                if w >= 1.0 {
                                    let (fill, op) = match status {
                                        "error" => ("var(--red)", "0.7"),
                                        "running" | "queued" => ("var(--accent)", "0.7"),
                                        _ => ("var(--green)", "0.5"),
                                    };
                                    bars.push(view! {
                                        <rect x={x1.to_string()} y={(y_center - 8.0).to_string()} width={w.to_string()} height="16" rx="4"
                                            fill={fill} opacity={op} />
                                    }.into_any());
                                }
                            }
                        } else if let Some(cs) = cs {
                            let first_idx = ((win_start - cs) / interval_s).ceil() as i32;
                            let last_idx = ((win_end - cs) / interval_s).floor() as i32;
                            for idx in first_idx..=last_idx {
                                let cycle_t = cs + idx as f64 * interval_s;
                                let cycle_end = cycle_t + interval_s;
                                if let Some(started) = agent_started {
                                    if cycle_end < started { continue; }
                                }
                                let is_current = cycle_t <= now && cycle_end > now;
                                let is_past = cycle_end <= now;
                                let is_latest = idx == 0;
                                if is_past && !is_latest { continue; }
                                let bar_end_ts = cycle_completed.unwrap_or(cycle_end).min(cycle_end);
                                let (bar_start, bar_end, cls) = if is_past {
                                    match status {
                                        "idle" | "done" => (cycle_t, bar_end_ts, "done"),
                                        "error" => (cycle_t, bar_end_ts, "error"),
                                        _ => continue,
                                    }
                                } else if is_current {
                                    let agent_alive = matches!(status, "running" | "queued");
                                    let in_cycle = cycle_t <= cs && cs < cycle_end;
                                    if !agent_alive && !in_cycle { continue; }
                                    match status {
                                        "idle" | "done" => (cycle_t, bar_end_ts, "done"),
                                        "error" => (cycle_t, bar_end_ts, "error"),
                                        _ => (cycle_t, now, "running"),
                                    }
                                } else {
                                    (cycle_t, cycle_t + interval_s * 0.1, "future")
                                };
                                let x1 = t2x(bar_start.max(win_start)).max(tx);
                                let x2 = t2x(bar_end.min(win_end)).min(tx + tw);
                                let w = (x2 - x1).max(2.0);
                                if w < 1.0 { continue; }
                                let bar_y = y_center - 8.0;
                                let bar_h = 16.0;
                                let rect = match cls {
                                    "future" => view! {
                                        <rect x={x1.to_string()} y={bar_y.to_string()} width={w.to_string()} height={bar_h.to_string()} rx="4"
                                            fill="var(--accent)" opacity="0.15" stroke="var(--accent)" stroke-width="1" stroke-dasharray="3,3" />
                                    }.into_any(),
                                    "running" => view! {
                                        <rect x={x1.to_string()} y={bar_y.to_string()} width={w.to_string()} height={bar_h.to_string()} rx="4"
                                            fill="var(--accent)" opacity="0.7" />
                                    }.into_any(),
                                    "error" => view! {
                                        <rect x={x1.to_string()} y={bar_y.to_string()} width={w.to_string()} height={bar_h.to_string()} rx="4"
                                            fill="var(--red)" opacity="0.7" />
                                    }.into_any(),
                                    _ => view! {
                                        <rect x={x1.to_string()} y={bar_y.to_string()} width={w.to_string()} height={bar_h.to_string()} rx="4"
                                            fill="var(--green)" opacity="0.5" />
                                    }.into_any(),
                                };
                                bars.push(rect);
                            }
                            // Render saved completed cycles (prev_cycles) for each agent in this row
                            let prev_map = state.prev_cycles.get();
                            for agent in &ags {
                                if let Some(entries) = prev_map.get(&agent.id) {
                                    for (pcs, pcc, pst) in entries {
                                        let parse = |s: &str| -> Option<f64> {
                                            if s.is_empty() { return None; }
                                            let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(s));
                                            let t = d.get_time() / 1000.0;
                                            if t.is_nan() || t <= 0.0 { None } else { Some(t) }
                                        };
                                        if let (Some(start), Some(end)) = (parse(pcs), parse(pcc)) {
                                            let end = end.min(start + interval_s);
                                            if end >= win_start && start <= win_end {
                                                let x1 = t2x(start.max(win_start)).max(tx);
                                                let x2 = t2x(end.min(win_end)).min(tx + tw);
                                                let w = (x2 - x1).max(2.0);
                                                if w >= 1.0 {
                                                    let cls = if pst == "error" { "error" } else { "done" };
                                                    let (fill, op) = match cls {
                                                        "error" => ("var(--red)", "0.7"),
                                                        _ => ("var(--green)", "0.5"),
                                                    };
                                                    bars.push(view! {
                                                        <rect x={x1.to_string()} y={(y_center - 8.0).to_string()} width={w.to_string()} height="16" rx="4"
                                                            fill={fill} opacity={op} />
                                                    }.into_any());
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        } else if !ags.is_empty() {
                            let x1 = tx;
                            let x2 = t2x(now);
                            bars.push(view! {
                                <rect x={x1.to_string()} y={(y_center - 8.0).to_string()} width={(x2 - x1).max(8.0).to_string()} height="16" rx="4"
                                    fill="var(--accent)" opacity="0.5" />
                            }.into_any());
                        } else {
                            bars.push(view! {
                                <rect x={tx.to_string()} y={(y_center - 8.0).to_string()} width={tw.to_string()} height="16" rx="4"
                                    fill="none" stroke="var(--muted)" stroke-width="1" stroke-dasharray="3,3" opacity="0.3" />
                            }.into_any());
                        };

                        let aid = ags.first().map(|a| a.id.clone());
                        let click_rect = aid.map(|id| {
                            let id_sel = id.clone();
                            view! {
                                <rect x="0" y={y.to_string()} width={w.to_string()} height={RH.to_string()} fill="transparent" class="clickable"
                                    on:click=move |ev| { ev.stop_propagation(); state.selected.set(Some(id_sel.clone())); state.editing_def.set(false); } />
                            }.into_any()
                        });

                        let is_manual = interval_mins == 0;
                        let text_y_offset = if is_manual { 12.0 } else { 4.0 };
                        view! {
                            <g>
                                <text x={LX.to_string()} y={(y_center + text_y_offset).to_string()} fill="var(--fg)" font-size="12" font-weight="bold">{label}</text>
                                {if is_manual {
                                    None
                                } else {
                                    Some(view! { <text x={(LX + 4.0).to_string()} y={(y_center + 18.0).to_string()} fill="var(--muted)" font-size="9">{format!("⏲ {}m", interval_mins)}</text> })
                                }}
                                {ags.first().map(|a| view! {
                                    <text x={(LX + 4.0).to_string()} y={(y_center + if is_manual { 24.0 } else { 30.0 }).to_string()} fill="var(--muted)" font-size="9">{a.id.clone()}</text>
                                })}
                                <line x1={tx.to_string()} y1={y_center.to_string()} x2={(tx + tw).to_string()} y2={y_center.to_string()} stroke="var(--border)" stroke-width="1" opacity="0.3" />
                                {bars.into_iter().collect_view()}
                                {click_rect}
                            </g>
                        }.into_any()
                    }).collect();

                    // ── now line ──
                    let now_x = t2x(now);
                    let footer_y = HH + n as f64 * RH + 4.0;
                    let ns = nonscheduled();
                    let groups: Vec<_> = std::iter::once(
                        view! { <line x1="0" y1={HH.to_string()} x2={w.to_string()} y2={HH.to_string()} stroke="var(--border)" stroke-width="1" /> }.into_any()
                    ).chain(
                        axis.into_iter()
                    ).chain(
                        row_views.into_iter()
                    ).chain(
                        std::iter::once(
                            view! {
                                <line x1={now_x.to_string()} y1={HH.to_string()} x2={now_x.to_string()} y2={(HH + n as f64 * RH).to_string()}
                                    stroke="var(--red)" stroke-width="2" stroke-dasharray="4,4" opacity="0.8" />
                            }.into_any()
                        )
                    ).chain(
                        std::iter::once(
                            view! {
                                <text x={tx.to_string()} y={footer_y.to_string()} fill="var(--muted)" font-size="11">
                                    {if ns > 0 { format!("{ns} on-demand agent(s) running") } else { String::new() }}
                                </text>
                            }.into_any()
                        )
                    ).collect();
                    groups.into_iter().collect_view().into_any()
                }}
            </svg>
        </div>
    }
}

#[component]
fn Detail(state: State) -> impl IntoView {
    move || match state.selected.get() {
        None => view! { <h3 class="muted placeholder">"select an agent to view its trace"</h3> }
            .into_any(),
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
                    {lines.into_iter()
                        .map(|l| view! { <div class=format!("line {}", l.class)>{l.text}</div> })
                        .collect_view()}
                </div>
            }
            .into_any()
        }
    }
}

// --------------------------------------------------------------------------
// Tools sidebar for agent spawn window
// --------------------------------------------------------------------------

#[component]
fn ToolsSidebar(state: State) -> impl IntoView {
    let builtin_tools = vec![
        ("write_file", "Write File", "Create or overwrite files"),
        ("read_file", "Read File", "Read file contents"),
        ("delete_file", "Delete File", "Delete files from the filesystem"),
        ("run_command", "Run Command", "Execute shell commands"),
        ("web_search", "Web Search", "Search the web for information"),
        ("add_task", "Add Task", "Add tasks to the task queue"),
        ("spawn_agent", "Spawn Agent", "Spawn other agents"),
    ];

    let mcp_tools = state.mcp_tools.get();

    let toggle_tool = move |tool_name: String| {
        if let Some(mut def) = state.ed_def.get() {
            let mut tools = def.tools.clone();
            if tools.contains(&tool_name) {
                tools.retain(|t| t != &tool_name);
            } else {
                tools.push(tool_name);
            }
            def.tools = tools.clone();
            let id = def.id.clone();
            state.ed_def.set(Some(def.clone()));
            state.ed_md.set(agent_def_to_md(&def));
            if !id.is_empty() {
                spawn_local(async move {
                    let _ = api::update_def(&id, &def).await;
                    state.teams.set(api::fetch_teams().await);
                });
            }
        }
    };

    let toggle_group_tools = move |server_name: String, select: bool| {
        if let Some(mut def) = state.ed_def.get() {
            let mut tools = def.tools.clone();
            let mcp_tools = state.mcp_tools.get();
            let group_tool_names: Vec<String> = mcp_tools.into_iter()
                .filter(|t| t.server_name == server_name)
                .map(|t| t.name)
                .collect();
            
            for name in group_tool_names {
                if select {
                    if !tools.contains(&name) {
                        tools.push(name);
                    }
                } else {
                    tools.retain(|t| t != &name);
                }
            }
            
            def.tools = tools.clone();
            let id = def.id.clone();
            state.ed_def.set(Some(def.clone()));
            state.ed_md.set(agent_def_to_md(&def));
            if !id.is_empty() {
                spawn_local(async move {
                    let _ = api::update_def(&id, &def).await;
                    state.teams.set(api::fetch_teams().await);
                });
            }
        }
    };

    let expanded_groups = RwSignal::new(std::collections::HashSet::<String>::new());

    view! {
        <div class="tools-sidebar-overlay" on:click=move |_| state.tools_sidebar_visible.set(false)>
            <div class="tools-sidebar" on:click=move |ev| ev.stop_propagation()>
                <div class="tools-sidebar-header">
                    <h3>"Available Tools"</h3>
                    <button on:click=move |_| state.tools_sidebar_visible.set(false)>"✕"</button>
                </div>
                <div class="tools-sidebar-content">
                    <div class="tool-section">
                        <h4 class="tool-section-title">"Built-in Tools"</h4>
                        {builtin_tools.into_iter().map(|(id, name, desc)| {
                            let is_selected = move || state.ed_def.get().map(|d| d.tools.contains(&id.to_string())).unwrap_or(false);
                            view! {
                                <label class="tool-item" class:selected=is_selected>
                                    <input type="checkbox"
                                        checked=is_selected
                                        on:change=move |_| toggle_tool(id.to_string()) />
                                    <div class="tool-info">
                                        <span class="tool-name">{name}</span>
                                        <span class="tool-desc">{desc}</span>
                                    </div>
                                </label>
                            }
                        }).collect_view()}
                    </div>
                    {if !mcp_tools.is_empty() {
                        let mut grouped_mcp: std::collections::BTreeMap<String, Vec<api::McpTool>> = std::collections::BTreeMap::new();
                        for tool in mcp_tools {
                            let server = if tool.server_name.is_empty() { "mcp".to_string() } else { tool.server_name.clone() };
                            grouped_mcp.entry(server).or_default().push(tool);
                        }

                        view! {
                            <div class="tool-section">
                                <h4 class="tool-section-title">"MCP Tools"</h4>
                                {grouped_mcp.into_iter().map(|(server_name, group_tools)| {
                                    let server_name_for_arrow = server_name.clone();
                                    let server_name_for_open = server_name.clone();
                                    let server_name_for_open2 = server_name.clone();
                                    
                                    let is_expanded_for_arrow = move || expanded_groups.get().contains(&server_name_for_open);
                                    let is_expanded_for_children = move || expanded_groups.get().contains(&server_name_for_open2);
                                    
                                    let toggle_expand = {
                                        let server_name = server_name.clone();
                                        move |ev: leptos::ev::MouseEvent| {
                                            ev.stop_propagation();
                                            expanded_groups.update(|set| {
                                                if set.contains(&server_name) {
                                                    set.remove(&server_name);
                                                } else {
                                                    set.insert(server_name.clone());
                                                }
                                            });
                                        }
                                    };

                                    let group_tools_for_all = group_tools.clone();
                                    let is_all_selected = move || {
                                        let tools_in_def = state.ed_def.get().map(|d| d.tools.clone()).unwrap_or_default();
                                        group_tools_for_all.iter().all(|t| tools_in_def.contains(&t.name))
                                    };

                                    let group_tools_for_any = group_tools.clone();
                                    let group_tools_for_all_ind = group_tools.clone();
                                    let is_indeterminate = move || {
                                        let tools_in_def = state.ed_def.get().map(|d| d.tools.clone()).unwrap_or_default();
                                        let any = group_tools_for_any.iter().any(|t| tools_in_def.contains(&t.name));
                                        let all = group_tools_for_all_ind.iter().all(|t| tools_in_def.contains(&t.name));
                                        any && !all
                                    };

                                    let group_tools_for_count = group_tools.clone();
                                    let selected_count = move || {
                                        let tools_in_def = state.ed_def.get().map(|d| d.tools.clone()).unwrap_or_default();
                                        group_tools_for_count.iter().filter(|t| tools_in_def.contains(&t.name)).count()
                                    };

                                    let group_tools_for_toggle = group_tools.clone();
                                    let toggle_all_in_group = {
                                        let server_name = server_name.clone();
                                        move |ev: leptos::ev::Event| {
                                            ev.stop_propagation();
                                            let tools_in_def = state.ed_def.get().map(|d| d.tools.clone()).unwrap_or_default();
                                            let all_selected = group_tools_for_toggle.iter().all(|t| tools_in_def.contains(&t.name));
                                            let target_state = !all_selected;
                                            toggle_group_tools(server_name.clone(), target_state);
                                        }
                                    };

                                    let group_tools_for_children = group_tools.clone();

                                    view! {
                                        <div class="tool-group">
                                            <div class="tool-group-header" on:click=toggle_expand>
                                                <div class="tool-group-title">
                                                    <span class="tool-group-arrow">
                                                        {move || if is_expanded_for_arrow() { "▼" } else { "▶" }}
                                                    </span>
                                                    <span>{server_name_for_arrow.clone()} " (" {selected_count} ")"</span>
                                                </div>
                                                <input type="checkbox"
                                                    checked=is_all_selected
                                                    prop:indeterminate=is_indeterminate
                                                    on:click=move |ev| ev.stop_propagation()
                                                    on:change=toggle_all_in_group />
                                            </div>
                                            {move || is_expanded_for_children().then(|| {
                                                view! {
                                                    <div class="tool-group-children">
                                                        {group_tools_for_children.clone().into_iter().map(|tool| {
                                                            let id = tool.name.clone();
                                                            let name = tool.name.clone();
                                                            let desc = tool.description.clone();
                                                            let id_for_check = id.clone();
                                                            let id_for_toggle = id.clone();
                                                            let is_selected = move || state.ed_def.get().map(|d| d.tools.contains(&id)).unwrap_or(false);
                                                            let is_checked = move || state.ed_def.get().map(|d| d.tools.contains(&id_for_check)).unwrap_or(false);
                                                            view! {
                                                                <label class="tool-item" class:selected=is_selected>
                                                                    <input type="checkbox"
                                                                        checked=is_checked
                                                                        on:change=move |_| toggle_tool(id_for_toggle.clone()) />
                                                                    <div class="tool-info">
                                                                        <span class="tool-name">{name}</span>
                                                                        <span class="tool-desc">{desc}</span>
                                                                    </div>
                                                                </label>
                                                            }
                                                        }).collect_view()}
                                                    </div>
                                                }
                                            })}
                                        </div>
                                    }
                                }).collect_view()}
                            </div>
                        }.into_any()
                    } else {
                        view! {}.into_any()
                    }}
                </div>
            </div>
        </div>
    }
}

// --------------------------------------------------------------------------
// Teams view (configure reusable agents)
// --------------------------------------------------------------------------

fn blank_def(team_id: String) -> AgentDef {
    AgentDef {
        id: String::new(),
        team_id,
        name: String::new(),
        model: None,
        instructions: String::new(),
        tools: Vec::new(),
        policy: "auto_approve".to_string(),
        memory_window: None,
        max_turns: None,
        schedule_mins: None,
        task: None,
        task_hint: None,
        builtin: false,
    }
}

// --------------------------------------------------------------------------
// Tasks view
// --------------------------------------------------------------------------

fn tasks_view(state: State) -> impl IntoView {
    view! {
        <main>
            <div class="left">
                <div class="row toolbar">
                    <h3 class="section">"TASKS"</h3>
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
    let selected = {
        let id = id.clone();
        move || state.task_selected.get().as_deref() == Some(id.as_str())
    };
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
        None => view! { <h3 class="muted placeholder">"select a task to review"</h3> }.into_any(),
        Some(id) => {
            let task = state.all_tasks().into_iter().find(|t| t.id == id);
            let status = task.as_ref().map(|t| t.status.clone()).unwrap_or_default();
            let is_proposed = status == "proposed";
            let id_save = id.clone();

            let accept = {
                let id = id.clone();
                let st = state;
                move |_| {
                    let id = id.clone();
                    spawn_local(async move {
                        let body = st.edit_body.get_untracked();
                        let teams = st.teams.get_untracked();
                        let team_id = teams
                            .iter()
                            .find(|t| !t.team.builtin)
                            .map(|t| t.team.id.clone())
                            .unwrap_or_else(|| "custom".to_string());
                        let def = md_to_agent_def(&body, &blank_def(team_id.clone()));
                        if !def.name.is_empty() {
                            let _ = api::create_def(&team_id, &def).await;
                            st.teams.set(api::fetch_teams().await);
                        }
                        let _ = api::update_task(
                            &id,
                            &st.edit_title.get_untracked(),
                            &st.edit_body.get_untracked(),
                            Some("implemented"),
                        )
                        .await;
                        st.refresh_tasks();
                    });
                }
            };

            let reject = {
                let id = id.clone();
                let st = state;
                move |_| {
                    let id = id.clone();
                    spawn_local(async move {
                        let _ = api::update_task(
                            &id,
                            &st.edit_title.get_untracked(),
                            &st.edit_body.get_untracked(),
                            Some("rejected"),
                        )
                        .await;
                        st.refresh_tasks();
                    });
                }
            };

            view! {
                <div class="row toolbar">
                    <h3>{move || state.edit_title.get()}</h3>
                    <span class=format!("badge {status}")>{status.clone()}</span>
                    <span class="spacer"></span>
                    {is_proposed.then(|| view! {
                        <button class="btn-accept" on:click=accept>"Accept"</button>
                        <button class="btn-reject" on:click=reject>"Reject"</button>
                    })}
                    <button on:click=move |_| state.raw_mode.update(|r| *r = !*r)>
                        {move || if state.raw_mode.get() { "View" } else { "Edit" }}</button>
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
                        <button class="mt-sm" on:click=move |_| {
                            let id = id_save.clone();
                            spawn_local(async move {
                                let _ = api::update_task(&id, &state.edit_title.get_untracked(),
                                    &state.edit_body.get_untracked(), None).await;
                                state.refresh_tasks();
                            });
                        }>"Save"</button>
                    }.into_any()
                } else {
                    view! {
                        <div class="md-preview" inner_html=move || render_agent_md(&state.edit_body.get())></div>
                    }.into_any()
                }}
            }.into_any()
        }
    }
}

fn md_to_html(src: &str) -> String {
    use pulldown_cmark::{html, Options, Parser};
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    let parser = Parser::new_ext(src, options);
    let mut out = String::new();
    html::push_html(&mut out, parser);
    out
}

/// Render an agent definition markdown (with YAML frontmatter) to HTML.
/// Strips frontmatter, renders metadata as a compact section, then the body.
fn render_agent_md(md: &str) -> String {
    let trimmed = md.trim();
    if trimmed.is_empty() {
        return "<div class=\"muted placeholder\">No content</div>".to_string();
    }
    if !trimmed.starts_with("---") {
        return format!("<div class=\"md-body\">{}</div>", md_to_html(trimmed));
    }
    let after_first = &trimmed[3..];
    let end = after_first.find("\n---").unwrap_or(after_first.len());
    let frontmatter = &after_first[..end];
    let body = if end < after_first.len() {
        after_first[end + 4..].trim()
    } else {
        ""
    };

    let mut meta = String::new();
    let mut tools = Vec::new();
    let mut schedule = String::new();
    let mut task = String::new();
    let mut task_hint = String::new();
    let mut in_tools = false;

    for line in frontmatter.lines() {
        let tl = line.trim();

        if in_tools {
            if tl.starts_with('#') {
                continue;
            }
            if let Some(item) = tl.strip_prefix("- ") {
                tools.push(item.trim().to_string());
                continue;
            }
            in_tools = false;
        }

        if tl.is_empty() || tl.starts_with('#') {
            continue;
        }

        if let Some((key, val)) = tl.split_once(':') {
            let k = key.trim();
            let v = val.split('#').next().unwrap_or("").trim();
            match k {
                "name" if !v.is_empty() => {
                    meta.push_str(&format!("<span class=\"meta-name\">{}</span> ", v));
                }
                "model" if !v.is_empty() => {
                    meta.push_str(&format!("<span class=\"meta-badge\">{}</span> ", v));
                }
                "policy" if !v.is_empty() => {
                    meta.push_str(&format!("<span class=\"meta-badge\">{}</span> ", v));
                }
                "tools" if v.is_empty() => {
                    in_tools = true;
                }
                "tools" => {
                    if v.starts_with('[') && v.ends_with(']') {
                        let inner = &v[1..v.len() - 1];
                        tools = inner
                            .split(',')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect();
                    }
                }
                "memory_window" if !v.is_empty() => {
                    meta.push_str(&format!("<span class=\"meta-badge\">mem: {}</span> ", v));
                }
                "max_turns" if !v.is_empty() => {
                    meta.push_str(&format!("<span class=\"meta-badge\">turns: {}</span> ", v));
                }
                "schedule_mins" if !v.is_empty() => {
                    if let Ok(n) = v.parse::<u64>() {
                        schedule = format!("{n}m");
                    }
                }
                "task" if !v.is_empty() && v != "|" => {
                    task = v.to_string();
                }
                "task_hint" if !v.is_empty() => {
                    task_hint = v.to_string();
                }
                _ => {}
            }
        }
    }

    if !tools.is_empty() {
        meta.push_str(&format!(
            "<span class=\"meta-badge\">tools: {}</span> ",
            tools.join(", ")
        ));
    }
    if !schedule.is_empty() {
        meta.push_str(&format!(
            "<span class=\"meta-badge\">⏲ {}</span> ",
            schedule
        ));
    }
    if !task.is_empty() {
        meta.push_str(&format!(
            "<span class=\"meta-badge\">task: {}</span> ",
            task
        ));
    }
    if !task_hint.is_empty() {
        meta.push_str(&format!(
            "<span class=\"meta-badge\">hint: {}</span> ",
            task_hint
        ));
    }

    let mut html = String::from("<div class=\"agent-meta\">");
    html.push_str(&meta);
    html.push_str("</div>\n<div class=\"md-body\">");
    html.push_str(&md_to_html(body));
    html.push_str("</div>");
    html
}

fn agent_def_to_md(def: &AgentDef) -> String {
    // Blank def → full template with all options shown.
    if def.name.is_empty() && def.id.is_empty() {
        return "\
---
name: 
model:  # leave empty for default
tools:
  - write_file
  # - delete_file
  # - run_command
  # - web_search
  # - add_task
  # - spawn_agent
policy: auto_approve  # auto_approve or deny_destructive
memory_window: 20  # conversation window
max_turns: 10  # max turns before stopping
schedule_mins:  # set to run on N-minute interval (blank = on-demand)
task:  # recurring task description (if scheduled)
task_hint:  # placeholder shown in the spawn task textarea (optional)
---

# Agent instructions

Describe what this agent should do...
"
        .to_string();
    }

    let mut fm = String::new();
    fm.push_str("---\n");
    fm.push_str(&format!("name: {}\n", def.name));
    if let Some(ref m) = def.model {
        fm.push_str(&format!("model: {m}\n"));
    }
    if !def.tools.is_empty() {
        fm.push_str("tools:\n");
        for t in &def.tools {
            fm.push_str(&format!("  - {t}\n"));
        }
    }
    fm.push_str(&format!("policy: {}\n", def.policy));
    if let Some(n) = def.memory_window {
        fm.push_str(&format!("memory_window: {n}\n"));
    }
    if let Some(n) = def.max_turns {
        fm.push_str(&format!("max_turns: {n}\n"));
    }
    if let Some(n) = def.schedule_mins {
        fm.push_str(&format!("schedule_mins: {n}\n"));
    }
    if let Some(ref t) = def.task {
        fm.push_str("task: |\n");
        for line in t.lines() {
            fm.push_str(&format!("  {line}\n"));
        }
    }
    if let Some(ref h) = def.task_hint {
        fm.push_str(&format!("task_hint: {h}\n"));
    }
    fm.push_str("---\n\n");
    fm.push_str(&def.instructions);
    fm
}

fn md_to_agent_def(md: &str, fallback: &AgentDef) -> AgentDef {
    let mut def = AgentDef {
        id: fallback.id.clone(),
        team_id: fallback.team_id.clone(),
        name: fallback.name.clone(),
        model: None,
        instructions: String::new(),
        tools: Vec::new(),
        policy: "auto_approve".to_string(),
        memory_window: None,
        max_turns: None,
        schedule_mins: None,
        task: None,
        task_hint: None,
        builtin: fallback.builtin,
    };

    // Find the frontmatter between --- delimiters.
    let trimmed = md.trim();
    if !trimmed.starts_with("---") {
        def.instructions = trimmed.to_string();
        return def;
    }
    let after_first = &trimmed[3..];
    let end = after_first.find("\n---").unwrap_or(after_first.len());
    let frontmatter = &after_first[..end];
    let body = if end < after_first.len() {
        after_first[end + 4..].trim()
    } else {
        ""
    };

    // Parse YAML-like frontmatter.
    let mut tools: Vec<String> = Vec::new();
    let mut in_tools = false;
    let mut task_body = String::new();
    let mut in_task = false;

    for line in frontmatter.lines() {
        let trimmed_line = line.trim();

        if in_task {
            if line.starts_with("  ") || trimmed_line.is_empty() {
                if !task_body.is_empty() {
                    task_body.push('\n');
                }
                task_body.push_str(trimmed_line.trim());
                continue;
            } else {
                def.task = Some(task_body.trim().to_string());
                task_body.clear();
                in_task = false;
            }
        }

        if in_tools {
            if trimmed_line.starts_with('#') {
                continue;
            }
            if let Some(item) = trimmed_line.strip_prefix("- ") {
                tools.push(item.trim().to_string());
                continue;
            } else {
                def.tools = tools.clone();
                tools.clear();
                in_tools = false;
            }
        }

        if trimmed_line.is_empty() || trimmed_line.starts_with('#') {
            continue;
        }

        if let Some((key, val)) = trimmed_line.split_once(':') {
            let k = key.trim();
            let v = val.split('#').next().unwrap_or("").trim();
            match k {
                "name" => def.name = v.to_string(),
                "model" => {
                    if !v.is_empty() {
                        def.model = Some(v.to_string());
                    }
                }
                "tools" => {
                    if v.is_empty() || v == "[]" {
                        // Block list follows
                        in_tools = true;
                    } else if v.starts_with('[') && v.ends_with(']') {
                        // Inline list [a, b, c]
                        let inner = &v[1..v.len() - 1];
                        def.tools = inner
                            .split(',')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect();
                    }
                }
                "policy" => def.policy = v.to_string(),
                "memory_window" => def.memory_window = v.parse().ok(),
                "max_turns" => def.max_turns = v.parse().ok(),
                "schedule_mins" => def.schedule_mins = v.parse().ok(),
                "task_hint" => {
                    if !v.is_empty() {
                        def.task_hint = Some(v.to_string());
                    }
                }
                "task" => {
                    if v == "|" {
                        in_task = true;
                    } else if !v.is_empty() {
                        def.task = Some(v.to_string());
                    }
                }
                _ => {}
            }
        }
    }

    // Flush remaining multi-line values.
    if !tools.is_empty() {
        def.tools = tools;
    }
    if in_task && !task_body.is_empty() {
        def.task = Some(task_body.trim().to_string());
    }

    def.instructions = body.to_string();
    def
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
            "ModelChanged" => {
                let st = state;
                spawn_local(async move {
                    st.model.set(api::fetch_model().await);
                    st.models.set(api::fetch_models().await);
                });
            }
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
        if status == "running" {
            state.agents.with(|ags| {
                if let Some(a) = ags.iter().find(|a| a.id == id) {
                    if !a.cycle_completed.is_empty() {
                        state.prev_cycles.update(|m| {
                            let entry = m.entry(id.clone()).or_default();
                            entry.push((
                                a.cycle_started.clone(),
                                a.cycle_completed.clone(),
                                a.status.clone(),
                            ));
                            if entry.len() > 5 {
                                entry.remove(0);
                            }
                        });
                    }
                }
            });
        }
        if status == "done" || status == "error" {
            let now_iso = js_sys::Date::new_0().to_iso_string();
            state.agents.update(|ags| {
                if let Some(a) = ags.iter_mut().find(|a| a.id == id) {
                    a.cycle_completed = now_iso.into();
                    a.status = status.to_string();
                }
            });
        } else {
            state.agents.update(|ags| {
                if let Some(a) = ags.iter_mut().find(|a| a.id == id) {
                    a.status = status.to_string();
                }
            });
        }
    }

    if let Some(line) = format_event(kind, data) {
        state
            .logs
            .update(|m| m.entry(id.clone()).or_default().push(line));
    }
}

fn format_event(kind: &str, d: &Value) -> Option<LogLine> {
    let s = |k: &str| d.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let (class, text) = match kind {
        "TaskStarted" => (
            "ev-turn",
            format!("▶ task started: {}", s("task_description")),
        ),
        "TurnStarted" => (
            "ev-turn",
            format!(
                "— turn {}/{}",
                d.get("turn_number").and_then(|v| v.as_u64()).unwrap_or(0) + 1,
                d.get("max_turns").and_then(|v| v.as_u64()).unwrap_or(0)
            ),
        ),
        "ToolCallRequested" => (
            "ev-tool",
            format!("🔧 {}({})", s("tool_name"), trunc(&s("arguments"), 200)),
        ),
        "ToolCallCompleted" => (
            "ev-tool",
            format!(
                "✓ {} → {}",
                s("tool_name"),
                trunc(
                    &d.get("result").map(|v| v.to_string()).unwrap_or_default(),
                    200
                )
            ),
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

// --------------------------------------------------------------------------
// Settings view
// --------------------------------------------------------------------------

fn settings_view(state: State) -> impl IntoView {
    view! {
        <main>
            <div class="left">
                <div class="row toolbar">
                    <h3 class="section">"SETTINGS"</h3>
                </div>
                <div class="agents">
                    <div class=move || if state.settings_opt.get() == SettingsOption::MCP { "agent sel" } else { "agent" }
                        style="cursor: pointer;"
                        on:click=move |_| state.settings_opt.set(SettingsOption::MCP)>
                        <div class="row">
                            <span class="id">"MCP Servers"</span>
                        </div>
                        <div class="task">"Configure external tool integrations"</div>
                    </div>
                </div>
            </div>
            <div class="right">
                {move || match state.settings_opt.get() {
                    SettingsOption::MCP => mcp_settings_pane(state).into_any(),
                }}
            </div>
        </main>
    }
}

fn mcp_settings_pane(state: State) -> impl IntoView {
    let (status_msg, set_status_msg) = RwSignal::new(String::new()).split();

    let start_edit = move |_| {
        let servers = state.mcp_servers.get_untracked();
        let json = serde_json::to_string_pretty(&serde_json::json!({ "mcpServers": servers }))
            .unwrap_or_else(|_| "{}".to_string());
        state.mcp_json_buffer.set(json);
        state.editing_mcp.set(true);
        set_status_msg.set(String::new());
    };

    let cancel_edit = move |_| {
        state.editing_mcp.set(false);
        set_status_msg.set(String::new());
    };

    let save_json = move |_| {
        let raw = state.mcp_json_buffer.get_untracked();
        let parsed: serde_json::Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => {
                set_status_msg.set(format!("Error: Invalid JSON ({})", e));
                return;
            }
        };

        let servers_val = if let Some(s) = parsed.get("mcpServers") {
            s.clone()
        } else {
            parsed
        };

        let servers: HashMap<String, api::McpServerConfig> = match serde_json::from_value(servers_val) {
            Ok(s) => s,
            Err(e) => {
                set_status_msg.set(format!("Error: Invalid MCP server format ({})", e));
                return;
            }
        };

        spawn_local(async move {
            match api::replace_all_mcp(servers).await {
                Ok(_) => {
                    state.mcp_servers.set(api::fetch_mcp().await);
                    state.editing_mcp.set(false);
                    set_status_msg.set("Configuration saved successfully".to_string());
                }
                Err(e) => {
                    set_status_msg.set(format!("Error saving configuration: {}", e));
                }
            }
        });
    };

    view! {
        <div class="config-panels" style="padding: 20px;">
            <div class="row toolbar" style="margin-bottom: 20px; padding: 0;">
                <h3 style="margin: 0;">"MCP Servers"</h3>
                <span class="spacer"></span>
                {move || if state.editing_mcp.get() {
                    view! {
                        <button class="btn-accept" on:click=save_json>"Save Changes"</button>
                        <button style="margin-left: 8px;" on:click=cancel_edit>"Cancel"</button>
                    }.into_any()
                } else {
                    view! {
                        <button on:click=start_edit>"Edit JSON"</button>
                    }.into_any()
                }}
            </div>

            {move || {
                let msg = status_msg.get();
                if msg.is_empty() { return None; }
                let is_err = msg.to_lowercase().contains("error");
                Some(view! {
                    <div class="status-box" style=format!("margin-bottom: 20px; padding: 12px; border-radius: 6px; border: 1px solid {}; background: {}; color: {}; font-size: 13px;",
                        if is_err { "var(--red)" } else { "var(--green)" },
                        if is_err { "#400" } else { "#040" },
                        if is_err { "#faa" } else { "#afa" })>
                        {msg}
                    </div>
                })
            }}

            {move || if state.editing_mcp.get() {
                view! {
                    <div class="spawn-section" style="border: 1px solid var(--border); padding: 16px; border-radius: 8px;">
                        <p style="font-size: 12px; color: var(--muted); margin-bottom: 12px;">
                            "Modify your MCP servers directly. Format follows the Claude Desktop configuration."
                        </p>
                        <div class="field">
                            <textarea 
                                style="min-height: 500px; font-family: monospace; font-size: 13px; line-height: 1.4; background: #111;"
                                prop:value=move || state.mcp_json_buffer.get()
                                on:input=move |e| state.mcp_json_buffer.set(event_target_value(&e))></textarea>
                        </div>
                    </div>
                }.into_any()
            } else {
                let servers = state.mcp_servers.get();
                if servers.is_empty() {
                    view! { <div class="muted placeholder">"No MCP servers configured. Click 'Edit JSON' to add one."</div> }.into_any()
                } else {
                    let mut sorted: Vec<_> = servers.into_iter().collect();
                    sorted.sort_by(|a, b| a.0.cmp(&b.0));
                    view! {
                        <div class="mcp-grid" style="display: flex; flex-direction: column; gap: 16px;">
                            {sorted.into_iter().map(|(id, cfg)| mcp_server_card(state, id, cfg)).collect_view()}
                        </div>
                    }.into_any()
                }
            }}
        </div>
    }
}

fn mcp_server_card(state: State, id: String, cfg: api::McpServerConfig) -> impl IntoView {
    let (show_env, set_show_env) = RwSignal::new(false).split();
    let (env_key, set_env_key) = RwSignal::new(String::new()).split();
    let (env_val, set_env_val) = RwSignal::new(String::new()).split();

    let id_del = id.clone();
    let delete_server = move |ev: leptos::ev::MouseEvent| {
        ev.stop_propagation();
        let id = id_del.clone();
        spawn_local(async move {
            let _ = api::delete_mcp(&id).await;
            state.mcp_servers.set(api::fetch_mcp().await);
        });
    };

    view! {
        <div class="agent" style="height: auto; padding-bottom: 12px;" on:click=move |_| set_show_env.update(|v| *v = !*v)>
            <div class="row">
                <span class="id" style="font-weight: bold; color: var(--accent);">{id.clone()}</span>
                <span class="spacer"></span>
                <button class="danger btn-small" on:click=delete_server>"remove"</button>
            </div>
            <div class="task" style="font-family: monospace; font-size: 11px;">
                {cfg.command.clone()} " " {cfg.args.join(" ")}
            </div>
            {
                let id = id.clone();
                let cfg = cfg.clone();
                move || show_env.get().then(|| {
                    let id_add = id.clone();
                    let cfg_add = cfg.clone();
                    let add_env = move |_| {
                        let key = env_key.get_untracked().trim().to_string();
                        let val = env_val.get_untracked().trim().to_string();
                        if key.is_empty() {
                            return;
                        }
                        let mut new_cfg = cfg_add.clone();
                        new_cfg.env.insert(key, val);
                        let id = id_add.clone();
                        spawn_local(async move {
                            let _ = api::upsert_mcp(&id, &new_cfg.command, new_cfg.args, new_cfg.env).await;
                            state.mcp_servers.set(api::fetch_mcp().await);
                            set_env_key.set(String::new());
                            set_env_val.set(String::new());
                        });
                    };

                    let id_list = id.clone();
                    let cfg_list = cfg.clone();
                    view! {
                        <div class="mcp-env-section" style="margin-top: 12px; border-top: 1px solid var(--border); padding-top: 12px;" on:click=move |ev| ev.stop_propagation()>
                            <h5 style="margin: 0 0 8px 0; font-size: 11px; text-transform: uppercase; color: var(--muted);">"Environment Variables / Secrets"</h5>
                            <div class="env-list">
                                {
                                    let id_list = id_list.clone();
                                    let cfg_list = cfg_list.clone();
                                    cfg_list.env.clone().into_iter().map(move |(k, v)| {
                                        let k_del = k.clone();
                                        let id_del = id_list.clone();
                                        let cfg_del = cfg_list.clone();
                                        view! {
                                            <div class="row env-row" style="margin-bottom: 4px; font-size: 12px;">
                                                <span style="font-family: monospace; color: var(--muted);">{k.clone()}</span>
                                                <span style="margin: 0 8px;">"="</span>
                                                <span style="font-family: monospace; flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;">
                                                    {if v.is_empty() { "(empty)" } else { "********" }}
                                                </span>
                                                <button class="danger btn-small" style="padding: 2px 6px;" on:click=move |_| {
                                                    let mut new_cfg = cfg_del.clone();
                                                    new_cfg.env.remove(&k_del);
                                                    let id = id_del.clone();
                                                    spawn_local(async move {
                                                        let _ = api::upsert_mcp(&id, &new_cfg.command, new_cfg.args, new_cfg.env).await;
                                                        state.mcp_servers.set(api::fetch_mcp().await);
                                                    });
                                                }>"×"</button>
                                            </div>
                                        }
                                    }).collect_view()
                                }
                            </div>
                            <div class="row mt-sm" style="gap: 4px;">
                                <input type="text" placeholder="KEY" style="flex: 1; height: 28px; font-size: 11px;"
                                    prop:value=move || env_key.get()
                                    on:input=move |e| set_env_key.set(event_target_value(&e)) />
                                <input type="password" placeholder="Value" style="flex: 1; height: 28px; font-size: 11px;"
                                    prop:value=move || env_val.get()
                                    on:input=move |e| set_env_val.set(event_target_value(&e)) />
                                <button class="btn-small" style="height: 28px;" on:click=add_env>"Add"</button>
                            </div>
                        </div>
                    }
                })
            }
        </div>
    }
}
