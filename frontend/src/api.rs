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
}

#[derive(Serialize)]
struct SpawnReq {
    task: String,
    policy: String,
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

pub async fn spawn_agent(task: String, policy: String) -> Result<(), String> {
    Request::post("/api/agents")
        .json(&SpawnReq { task, policy })
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
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

pub async fn update_task(id: &str, title: &str, body: &str) -> Result<(), String> {
    Request::put(&format!("/api/tasks/{id}"))
        .json(&serde_json::json!({ "title": title, "body": body }))
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub async fn accept_task(id: &str) {
    let _ = Request::post(&format!("/api/tasks/{id}/accept")).send().await;
}

pub async fn reject_task(id: &str) {
    let _ = Request::post(&format!("/api/tasks/{id}/reject")).send().await;
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
