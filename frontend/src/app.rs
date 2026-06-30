//! Leptos dashboard: an Agents view (spawn/monitor/cancel) and a Tasks view
//! (review/edit/accept/reject the task queue the pipeline produces).

use std::collections::HashMap;

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::Value;

use crate::api::{self, AgentDef, AgentInfo, SwarmEvent, Task, TeamWithAgents};

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
    teams: RwSignal<Vec<TeamWithAgents>>,
    models: RwSignal<Vec<String>>,
    editing_def: RwSignal<bool>,
    ed_md: RwSignal<String>,
    ed_def: RwSignal<Option<AgentDef>>,
    raw_mode_def: RwSignal<bool>,
    spawn_task: RwSignal<String>,
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
    };

    spawn_local(async move {
        state.model.set(api::fetch_model().await);
        state.tasks.set(api::fetch_tasks().await);
        state.history.set(api::fetch_history().await);
        let teams = api::fetch_teams().await;
        if !teams.iter().any(|t| t.team.name == "custom") {
            let _ = api::create_team("custom").await;
            state.teams.set(api::fetch_teams().await);
        } else {
            state.teams.set(teams);
        }
        state.models.set(api::fetch_models().await);
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
                    "Tasks"</button>
                <button class:active=move || state.tab.get() == ViewTab::Agents
                    on:click=move |_| { state.tab.set(ViewTab::Agents); state.editing_def.set(false); state.selected.set(None); }>"Agents"</button>
            </nav>
            <select class="model-select"
                on:change=move |ev| {
                    let name = event_target_value(&ev);
                    if !name.is_empty() {
                        let st = state;
                        spawn_local(async move {
                            let _ = api::set_model(&name).await;
                            st.model.set(name);
                        });
                    }
                }
                prop:value=move || state.model.get()>
                <option value="" disabled>"— select model —"</option>
                {move || state.models.get().into_iter().map(|m| {
                    let is_current = m == state.model.get();
                    view! {
                        <option value=m.clone() selected=is_current>{m.clone()}</option>
                    }
                }).collect_view()}
            </select>
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
        let def = state.ed_def.get().unwrap_or_else(|| blank_def("custom".to_string()));
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
        let id = state.ed_def
            .get()
            .and_then(|d| if d.id.is_empty() { None } else { Some(d.id.clone()) });
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
        if task.is_empty() { return; }
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
                <div class="row" style="margin:0 0 8px">
                    <h3 class="section" style="margin:0">"AGENTS"</h3>
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
                                            {ac.schedule_mins.map(|m| view! { <span class="badge sys">{format!("⏲ {m}m")}</span> })}
                                            {move || (active_count() > 0).then(|| view! {
                                                <span class="badge running">{"● ".to_string() + &active_count().to_string()}</span>
                                            })}
                                            <span class="spacer"></span>
                                            <span class="badge">{ac.model.clone().unwrap_or_else(|| "default".to_string())}</span>
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
                                    <button on:click=move |_| state.selected.set(None)>"← Back"</button>
                                    {active_agents_graph(state)}
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
                                <div class="row" style="margin-bottom:8px">
                                    <h3 style="margin:0">{name.clone()}</h3>
                                    <span class="spacer"></span>
                                    {ro.then(|| view! { <span class="badge sys">"read-only"</span> })}
                                    <button on:click=move |_| state.raw_mode_def.set(false)>"view"</button>
                                    <button on:click=move |_| { state.editing_def.set(false); }>"Close"</button>
                                </div>
                                <textarea class="md-edit" prop:value=move || state.ed_md.get()
                                    on:input=move |e| state.ed_md.set(event_target_value(&e))></textarea>
                                {(!ro).then(|| view! {
                                    <div class="row" style="margin-top:8px">
                                        <button on:click=move |_| { save(); }>"Save"</button>
                                        <button on:click=move |_| { state.raw_mode_def.set(false); state.editing_def.set(false); }>"Cancel"</button>
                                    </div>
                                })}
                            }.into_any()
                        } else {
                            view! {
                                <div class="row" style="margin-bottom:8px">
                                    <h3 style="margin:0">{name.clone()}</h3>
                                    <span class="spacer"></span>
                                    {ro.then(|| view! { <span class="badge sys">"read-only"</span> })}
                                    <button on:click=move |_| state.raw_mode_def.set(true)>"edit"</button>
                                    <button on:click=move |_| { state.editing_def.set(false); }>"Close"</button>
                                </div>
                                <div class="md-preview" style="height:calc(100vh - 230px)" inner_html=move || render_agent_md(&state.ed_md.get())></div>
                                <div class="spawn-section">
                                    {(!ro).then(|| view! {
                                        <div class="row">
                                            <button on:click=move |_| state.raw_mode_def.set(true)>"Edit YAML"</button>
                                            {state.ed_def.get().map(|d| !d.id.is_empty()).unwrap_or(false).then(|| view! {
                                                <button class="danger" on:click=del>"Delete"</button>
                                            })}
                                        </div>
                                    })}
                                    <div class="field" style="margin-top:8px">
                                        <textarea node_ref=spawn_input
                                            on:input=move |e| state.spawn_task.set(event_target_value(&e))
                                            placeholder=move || state.ed_def.get().and_then(|d| d.task_hint.clone()).unwrap_or_else(|| "Describe a task for this agent…".to_string())></textarea>
                                        <button style="margin-top:4px" on:click=do_spawn>"Spawn"</button>
                                    </div>
                                </div>
                            }.into_any()
                        }
                    } else {
                        active_agents_graph(state).into_any()
                    }
                }}
            </div>
        </main>
    }
}

fn active_agents_graph(state: State) -> impl IntoView {
    view! {
        <svg class="graph-svg" viewBox="0 0 340 340">
            {move || {
                let ags = state.agents.get();
                let n = ags.len();
                if n == 0 {
                    return view! { <text x="170" y="170" text-anchor="middle" fill="var(--muted)" font-size="14">"no active agents"</text> }.into_any();
                }
                let (cx, cy, r) = (170.0f64, 170.0f64, 130.0f64);
                ags.into_iter().enumerate().map(|(i, a)| {
                    let angle = std::f64::consts::PI * 2.0 * (i as f64) / (n as f64) - std::f64::consts::PI / 2.0;
                    let x = cx + r * angle.cos();
                    let y = cy + r * angle.sin();
                    let label = if a.id.len() > 12 { format!("{}…", &a.id[..12]) } else { a.id.clone() };
                    let fill = match a.status.as_str() {
                        "running" => "var(--accent)",
                        "done" => "var(--green)",
                        "error" => "var(--red)",
                        "queued" => "var(--yellow)",
                        "idle" => "#bc8cff",
                        _ => "var(--muted)",
                    };
                    let id_sel = a.id.clone();
                    let yt = y + 4.0;
                    view! {
                        <g on:click=move |_| { state.selected.set(Some(id_sel.clone())); state.editing_def.set(false); } style="cursor:pointer">
                            <circle cx={x.to_string()} cy={y.to_string()} r="18" fill=fill opacity="0.3" stroke=fill stroke-width="3" />
                            <text x={x.to_string()} y={yt.to_string()} text-anchor="middle" fill="var(--fg)" font-size="9" font-weight="bold">{label}</text>
                        </g>
                    }
                }).collect_view().into_any()
            }}
        </svg>
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
                        let team_id = teams.iter()
                            .find(|t| !t.team.builtin)
                            .map(|t| t.team.id.clone())
                            .unwrap_or_else(|| "custom".to_string());
                        let def = md_to_agent_def(&body, &blank_def(team_id.clone()));
                        if !def.name.is_empty() {
                            let _ = api::create_def(&team_id, &def).await;
                            st.teams.set(api::fetch_teams().await);
                        }
                        let _ = api::update_task(&id, &st.edit_title.get_untracked(),
                            &st.edit_body.get_untracked(), Some("implemented")).await;
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
                        let _ = api::update_task(&id, &st.edit_title.get_untracked(),
                            &st.edit_body.get_untracked(), Some("rejected")).await;
                        st.refresh_tasks();
                    });
                }
            };

            view! {
                <div class="row" style="margin-bottom:8px">
                    <h3 style="margin:0">{move || state.edit_title.get()}</h3>
                    <span class=format!("badge {status}")>{status.clone()}</span>
                    <span class="spacer"></span>
                    {is_proposed.then(|| view! {
                        <button style="color:var(--green);border-color:var(--green)" on:click=accept>"Accept"</button>
                        <button style="color:var(--red);border-color:var(--red)" on:click=reject>"Reject"</button>
                    })}
                    <button on:click=move |_| state.raw_mode.update(|r| *r = !*r)>
                        {move || if state.raw_mode.get() { "view" } else { "edit" }}</button>
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
    let parser = pulldown_cmark::Parser::new(src);
    let mut out = String::new();
    pulldown_cmark::html::push_html(&mut out, parser);
    out
}

/// Render an agent definition markdown (with YAML frontmatter) to HTML.
/// Strips frontmatter, renders metadata as a compact section, then the body.
fn render_agent_md(md: &str) -> String {
    let trimmed = md.trim();
    if !trimmed.starts_with("---") {
        return md_to_html(trimmed);
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
                        tools = inner.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
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
        meta.push_str(&format!("<span class=\"meta-badge\">tools: {}</span> ", tools.join(", ")));
    }
    if !schedule.is_empty() {
        meta.push_str(&format!("<span class=\"meta-badge\">⏲ {}</span> ", schedule));
    }
    if !task.is_empty() {
        meta.push_str(&format!("<span class=\"meta-badge\">task: {}</span> ", task));
    }
    if !task_hint.is_empty() {
        meta.push_str(&format!("<span class=\"meta-badge\">hint: {}</span> ", task_hint));
    }

    let mut html = String::from("<div class=\"agent-meta\">");
    html.push_str(&meta);
    html.push_str("</div>\n");
    html.push_str(&md_to_html(body));
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
".to_string();
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
                        def.tools = inner.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
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
