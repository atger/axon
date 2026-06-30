//! REST + WebSocket access to the axon `serve` backend, for the WASM dashboard.

use futures::StreamExt;
use gloo_net::http::Request;
use gloo_net::websocket::{Message, futures::WebSocket};
use leptos::task::spawn_local;
use serde::{Deserialize, Serialize};

/// Mirrors `crate::swarm::AgentInfo` on the server (snake_case enums as strings).
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct AgentInfo {
    pub id: String,
    pub task: String,
    pub model: String,
    pub policy: String,
    pub status: String,
     #[serde(default)]
    pub role: String,
     #[serde(default)]
    pub perpetual: bool,
     #[serde(default)]
    pub def_name: Option<String>,
     // Lifecycle tracking fields
     #[serde(default)]
    pub last_seen_stage: String,
     #[serde(default)]
    pub current_stage: String,
}

/// Mirrors `crate::swarm::teams::Team`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Team {
    pub id: String,
    pub name: String,
      #[serde(default)]
    pub builtin: bool,
}

/// Mirrors `crate::swarm::teams::AgentDef` (a saved agent configuration).
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct AgentDef {
     #[serde(default)]
    pub id: String,
     #[serde(default)]
    pub team_id: String,
    pub name: String,
      #[serde(default)]
    pub model: Option<String>,
      #[serde(default)]
    pub instructions: String,
      #[serde(default)]
    pub tools: Vec<String>,
      #[serde(default = "default_policy")]
    pub policy: String,
      #[serde(default)]
    pub memory_window: Option<usize>,
      #[serde(default)]
    pub max_turns: Option<usize>,
      #[serde(default)]
    pub schedule_mins: Option<u64>,
      #[serde(default)]
    pub task: Option<String>,
      #[serde(default)]
    pub task_hint: Option<String>,
      #[serde(default)]
    pub builtin: bool,
}

fn default_policy() -> String {
      "auto_approve".to_string()
}

/// Mirrors `crate::swarm::teams::TeamWithAgents`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct TeamWithAgents {
    pub team: Team,
     #[serde(default)]
    pub agents: Vec<AgentDef>,
}

/// Mirrors `crate::swarm::store::Task`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Task {
    pub id: String,
    pub title: String,
     #[serde(default)]
    pub description: String,
     #[serde(default)]
    pub body: String,
     #[serde(default)]
    pub tags: String,
    pub status: String,
     #[serde(default)]
    pub updated: String,
}

/// Mirrors `crate::swarm::SwarmEvent`: an agent id + the externally-tagged
/// AutoAgents protocol event as raw JSON.
#[derive(Debug, Deserialize)]
pub struct SwarmEvent {
    pub agent_id: String,
    pub event: serde_json::Value,
}

#[derive(Deserialize)]
struct ModelsResp {
    current: String,
     #[serde(default)]
    models: Vec<String>,
}

#[derive(Serialize)]
struct SpawnReq {
    def_id: String,
    task: String,
}

pub async fn fetch_agents() -> Vec<AgentInfo> {
    match Request::get("/api/agents").send().await {
        Ok(resp) => resp.json().await.unwrap_or_default(),
        Err(_) => Vec::new(),
     }
}

pub async fn fetch_model() -> String {
    match Request::get("/api/models").send().await {
        Ok(resp) => resp
             .json::<ModelsResp>()
             .await
             .map(|m| m.current)
             .unwrap_or_default(),
        Err(_) => String::new(),
     }
}

/// The list of selectable models (installed in Ollama; current model first).
pub async fn fetch_models() -> Vec<String> {
    match Request::get("/api/models").send().await {
        Ok(resp) => resp
             .json::<ModelsResp>()
             .await
             .map(|m| m.models)
             .unwrap_or_default(),
        Err(_) => Vec::new(),
     }
}

pub async fn spawn_agent(def_id: String, task: String) -> Result<(), String> {
    Request::post("/api/agents")
         .json(&SpawnReq { def_id, task })
         .map_err(|e| e.to_string())?
         .send()
         .await
         .map_err(|e| e.to_string())?;
    Ok(())
}

// -- teams & agent definitions ---------------------------------------------

pub async fn fetch_teams() -> Vec<TeamWithAgents> {
    match Request::get("/api/teams").send().await {
        Ok(resp) => resp.json().await.unwrap_or_default(),
        Err(_) => Vec::new(),
     }
}

pub async fn create_team(name: &str) -> Result<(), String> {
    Request::post("/api/teams")
         .json(&serde_json::json!({ "name": name }))
         .map_err(|e| e.to_string())?
         .send()
         .await
         .map_err(|e| e.to_string())?;
    Ok(())
}

pub async fn delete_team(id: &str) -> Result<(), String> {
    Request::delete(&format!("/api/teams/{id}"))
         .send()
         .await
         .map_err(|e| e.to_string())?;
    Ok(())
}

pub async fn create_def(team_id: &str, def: &AgentDef) -> Result<(), String> {
    Request::post(&format!("/api/teams/{team_id}/agents"))
         .json(def)
         .map_err(|e| e.to_string())?
         .send()
         .await
         .map_err(|e| e.to_string())?;
    Ok(())
}

pub async fn update_def(id: &str, def: &AgentDef) -> Result<(), String> {
    Request::put(&format!("/api/agent-defs/{id}"))
         .json(def)
         .map_err(|e| e.to_string())?
         .send()
         .await
         .map_err(|e| e.to_string())?;
    Ok(())
}

pub async fn delete_def(id: &str) -> Result<(), String> {
    Request::delete(&format!("/api/agent-defs/{id}"))
         .send()
         .await
         .map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Deserialize)]
struct GenerateResp {
    markdown: String,
}

pub async fn generate_def(prompt: &str) -> Result<String, String> {
    let resp = Request::post("/api/agent-defs/generate")
        .json(&serde_json::json!({ "prompt": prompt }))
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let body: GenerateResp = resp.json().await.map_err(|e| e.to_string())?;
    Ok(body.markdown)
}

pub async fn cancel_agent(id: String) {
    let _ = Request::delete(&format!("/api/agents/{id}")).send().await;
}

pub async fn cancel_all() {
    let _ = Request::post("/api/agents/cancel-all").send().await;
}

pub async fn fetch_tasks() -> Vec<Task> {
    match Request::get("/api/tasks").send().await {
        Ok(resp) => resp.json().await.unwrap_or_default(),
        Err(_) => Vec::new(),
     }
}

pub async fn fetch_history() -> Vec<Task> {
    match Request::get("/api/tasks/history").send().await {
        Ok(resp) => resp.json().await.unwrap_or_default(),
        Err(_) => Vec::new(),
     }
}

pub async fn update_task(id: &str, title: &str, body: &str, status: Option<&str>) -> Result<(), String> {
    let mut payload = serde_json::json!({ "title": title, "body": body });
    if let Some(s) = status {
        payload["status"] = serde_json::json!(s);
    }
    Request::put(&format!("/api/tasks/{id}"))
         .json(&payload)
         .map_err(|e| e.to_string())?
         .send()
         .await
         .map_err(|e| e.to_string())?;
    Ok(())
}

pub async fn set_model(name: &str) -> Result<(), String> {
    Request::post("/api/models")
        .json(&serde_json::json!({ "model": name }))
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Build the absolute `ws(s)://host/ws` URL from the current page location.
fn ws_url() -> String {
    let loc = web_sys::window().expect("window").location();
    let proto = if loc.protocol().as_deref() == Ok("https:") {
         "wss"
     } else {
         "ws"
     };
    let host = loc.host().unwrap_or_default();
    format!("{proto}://{host}/ws")
}

/// Open the `/ws` stream and call `on_event` for each event, reconnecting on drop.
pub fn connect_ws<F>(on_event: F)
where
    F: Fn(SwarmEvent) + 'static,
{
    spawn_local(async move {
        loop {
            if let Ok(mut ws) = WebSocket::open(&ws_url()) {
                while let Some(msg) = ws.next().await {
                    if let Ok(Message::Text(txt)) = msg
                         && let Ok(ev) = serde_json::from_str::<SwarmEvent>(&txt)
                     {
                        on_event(ev);
                     }
                 }
            }
             // Reconnect after a short delay.
            gloo_timers::future::TimeoutFuture::new(1500).await;
         }
    });
}
