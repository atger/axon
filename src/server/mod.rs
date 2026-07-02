//! HTTP + WebSocket server for the `axon serve` swarm dashboard.
//!
//! REST drives agent lifecycle (spawn / list / get / cancel); a WebSocket
//! streams AutoAgents protocol events (tagged by agent id) to the browser. The
//! Svelte dashboard is embedded from `frontend/dist` via `rust-embed`.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{
        Path, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{StatusCode, Uri, header},
    response::{Html, IntoResponse, Response},
    routing::{get, post, put, delete},
};

mod tasks;
mod teams;
use color_eyre::eyre::{Result, WrapErr};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::broadcast;

use crate::swarm::{Swarm, teams as defstore};
use autoagents::core::tool::ToolT;

#[derive(rust_embed::RustEmbed)]
#[folder = "frontend/dist"]
struct Assets;

/// Start the dashboard server, bound to `host:port`.
pub async fn run_server(swarm: Arc<Swarm>, host: String, port: u16) -> Result<()> {
    let app = Router::new()
        .route("/api/agents", get(list_agents).post(spawn_agent))
        .route("/api/agents/history", get(list_agent_history))
        .route("/api/agents/cancel-all", post(cancel_all))
        .route("/api/agents/:id", get(get_agent).delete(cancel_agent))
        .route("/api/teams", get(teams::list).post(teams::create))
        .route("/api/teams/:id", put(teams::rename).delete(teams::delete))
        .route("/api/teams/:id/agents", post(teams::create_def))
        .route("/api/agent-defs/:id", put(teams::update_def).delete(teams::delete_def))
        .route("/api/agent-defs/generate", post(teams::generate_def))
        .route("/api/models", get(list_models).post(set_model))
        .route("/api/mcp", get(list_mcp).post(upsert_mcp).put(replace_all_mcp))
        .route("/api/mcp/:id", delete(delete_mcp))
        .route("/api/mcp/tools", get(list_mcp_tools))
        .route("/api/tasks", get(tasks::list))
        .route("/api/tasks/history", get(tasks::history))
        .route("/api/tasks/:id", get(tasks::get).put(tasks::update))
        .route("/ws", get(ws_handler))
        .fallback(static_handler)
        .with_state(swarm);

    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .wrap_err_with(|| format!("invalid bind address {host}:{port}"))?;

    if !addr.ip().is_loopback() {
        eprintln!(
            "axon serve: WARNING binding to {addr} — the dashboard has NO AUTH and agents run \
             shell commands on this host. Only do this on a trusted network (or behind a reverse \
             proxy / SSH tunnel)."
        );
    }

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .wrap_err_with(|| format!("failed to bind {addr}"))?;
    eprintln!("axon serve: dashboard on http://{addr}");

    axum::serve(listener, app)
        .await
        .wrap_err("server error")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// REST handlers
// ---------------------------------------------------------------------------

async fn list_agents(State(swarm): State<Arc<Swarm>>) -> Json<serde_json::Value> {
    Json(json!(swarm.list().await))
}

async fn list_agent_history(State(swarm): State<Arc<Swarm>>) -> Json<serde_json::Value> {
    Json(json!(swarm.history().await))
}

#[derive(Deserialize)]
struct SpawnRequest {
    def_id: String,
    task: String,
}

async fn spawn_agent(
    State(swarm): State<Arc<Swarm>>,
    Json(req): Json<SpawnRequest>,
) -> Response {
    if req.task.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "task must not be empty").into_response();
    }
    let def = match defstore::resolve_def(&req.def_id) {
        Ok(Some(def)) => def,
        Ok(None) => return (StatusCode::BAD_REQUEST, "no such agent definition").into_response(),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };
    match swarm.spawn_from_def(def, req.task).await {
        Ok(id) => (StatusCode::CREATED, Json(json!({ "id": id }))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn get_agent(State(swarm): State<Arc<Swarm>>, Path(id): Path<String>) -> Response {
    match swarm.get(&id).await {
        Some(info) => Json(json!(info)).into_response(),
        None => (StatusCode::NOT_FOUND, "no such agent").into_response(),
    }
}

async fn cancel_agent(State(swarm): State<Arc<Swarm>>, Path(id): Path<String>) -> Response {
    if swarm.cancel(&id).await {
        StatusCode::NO_CONTENT.into_response()
    } else {
        (StatusCode::NOT_FOUND, "no such agent").into_response()
    }
}

async fn cancel_all(State(swarm): State<Arc<Swarm>>) -> StatusCode {
    swarm.cancel_all().await;
    StatusCode::NO_CONTENT
}

async fn list_models(State(swarm): State<Arc<Swarm>>) -> Json<serde_json::Value> {
    let current = swarm.model().await;
    let mut models = crate::llm::ollama::list_available(swarm.ollama_url()).await;
    // Ensure the current model is always selectable, even if `/api/tags` failed.
    if !models.iter().any(|m| m == &current) {
        models.insert(0, current.clone());
    }
    Json(json!({ "current": current, "models": models }))
}

#[derive(Deserialize)]
struct SetModelReq {
    model: String,
}

async fn set_model(
    State(swarm): State<Arc<Swarm>>,
    Json(req): Json<SetModelReq>,
) -> Response {
    if req.model.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "model must not be empty").into_response();
    }
    match swarm.set_model(&req.model).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn list_mcp() -> Json<serde_json::Value> {
    let config = crate::config::AxonConfig::load();
    Json(json!(config.mcp_servers))
}

#[derive(Deserialize)]
struct UpsertMcpReq {
    id: String,
    command: String,
    args: Vec<String>,
    #[serde(default)]
    env: std::collections::HashMap<String, String>,
}

async fn upsert_mcp(Json(req): Json<UpsertMcpReq>) -> Response {
    if req.id.trim().is_empty() || req.command.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "id and command must not be empty").into_response();
    }
    let mut config = crate::config::AxonConfig::load();
    config.mcp_servers.insert(
        req.id,
        crate::config::McpServerConfig {
            command: req.command,
            args: req.args,
            env: req.env,
        },
    );
    match config.save() {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn delete_mcp(Path(id): Path<String>) -> Response {
    let mut config = crate::config::AxonConfig::load();
    if config.mcp_servers.remove(&id).is_some() {
        match config.save() {
            Ok(()) => StatusCode::NO_CONTENT.into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response(),
        }
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

async fn replace_all_mcp(
    Json(servers): Json<std::collections::HashMap<String, crate::config::McpServerConfig>>,
) -> Response {
    let mut config = crate::config::AxonConfig::load();
    config.mcp_servers = servers;
    match config.save() {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn list_mcp_tools(State(swarm): State<Arc<Swarm>>) -> Json<serde_json::Value> {
    let tools = swarm.mcp_tools().await;
    let tool_list: Vec<serde_json::Value> = tools
        .iter()
        .map(|(server_name, t)| {
            json!({
                "name": t.name(),
                "description": t.description(),
                "server_name": server_name,
                "args_schema": t.args_schema(),
            })
        })
        .collect();
    Json(json!(tool_list))
}

// ---------------------------------------------------------------------------
// WebSocket: stream swarm events to the dashboard
// ---------------------------------------------------------------------------

async fn ws_handler(ws: WebSocketUpgrade, State(swarm): State<Arc<Swarm>>) -> Response {
    ws.on_upgrade(move |socket| ws_loop(socket, swarm))
}

async fn ws_loop(mut socket: WebSocket, swarm: Arc<Swarm>) {
    let mut rx = swarm.subscribe();
    loop {
        tokio::select! {
            ev = rx.recv() => match ev {
                Ok(ev) => {
                    let txt = match serde_json::to_string(&ev) {
                        Ok(t) => t,
                        Err(_) => continue,
                    };
                    if socket.send(Message::Text(txt)).await.is_err() {
                        break;
                    }
                }
                // Client too slow — drop missed events and keep going.
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            },
            msg = socket.recv() => match msg {
                Some(Ok(Message::Close(_))) | None => break,
                Some(Ok(_)) => { /* dashboard is receive-only for v1 */ }
                Some(Err(_)) => break,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Static assets (embedded SPA) with client-side-routing fallback
// ---------------------------------------------------------------------------

async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    if let Some(content) = Assets::get(path) {
        return ([(header::CONTENT_TYPE, content_type(path))], content.data).into_response();
    }
    // SPA fallback: serve index.html for unknown client routes.
    match Assets::get("index.html") {
        Some(content) => Html(content.data).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            "dashboard assets not built — run `npm run build` in frontend/",
        )
            .into_response(),
    }
}

/// Minimal extension → MIME mapping (avoids a mime_guess dependency).
fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("wasm") => "application/wasm",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}
