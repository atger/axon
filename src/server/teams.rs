//! REST handlers for teams and agent definitions (templates). Backed by the
//! SQLite store; the built-in "axon" team is read-only (mutations are rejected
//! in `crate::swarm::teams`).

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Deserialize;

use crate::swarm::Swarm;
use crate::swarm::teams::{self, DefForm};

pub async fn list() -> Response {
    match teams::all_teams() {
        Ok(items) => Json(items).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
pub struct NameReq {
    name: String,
}

pub async fn create(Json(req): Json<NameReq>) -> Response {
    match teams::add_team(&req.name) {
        Ok(team) => (StatusCode::CREATED, Json(team)).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

pub async fn rename(Path(id): Path<String>, Json(req): Json<NameReq>) -> Response {
    match teams::rename_team(&id, &req.name) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

pub async fn delete(State(swarm): State<Arc<Swarm>>, Path(id): Path<String>) -> Response {
    match teams::delete_team(&id) {
        Ok(()) => {
            swarm.resync_schedules().await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

pub async fn create_def(
    State(swarm): State<Arc<Swarm>>,
    Path(team_id): Path<String>,
    Json(form): Json<DefForm>,
) -> Response {
    match teams::add_def(&team_id, &form) {
        Ok(def) => {
            swarm.resync_schedules().await;
            (StatusCode::CREATED, Json(def)).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

pub async fn update_def(
    State(swarm): State<Arc<Swarm>>,
    Path(id): Path<String>,
    Json(form): Json<DefForm>,
) -> Response {
    match teams::update_def(&id, &form) {
        Ok(()) => {
            swarm.resync_schedules().await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

pub async fn delete_def(State(swarm): State<Arc<Swarm>>, Path(id): Path<String>) -> Response {
    match teams::delete_def(&id) {
        Ok(()) => {
            swarm.resync_schedules().await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
pub struct GenerateReq {
    prompt: String,
}

pub async fn generate_def(State(swarm): State<Arc<Swarm>>, Json(req): Json<GenerateReq>) -> Response {
    match swarm.generate_agent_def(&req.prompt).await {
        Ok(text) => Json(serde_json::json!({ "markdown": text })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e.to_string() }))).into_response(),
    }
}
