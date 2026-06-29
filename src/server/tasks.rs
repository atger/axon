//! REST handlers for the TASKS queue: list (active/history), read, edit,
//! accept, reject. Backed by the SQLite task store.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Deserialize;

use crate::swarm::{Swarm, store};

pub async fn list() -> Response {
    match store::list_active() {
        Ok(items) => Json(items).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub async fn history() -> Response {
    match store::list_history() {
        Ok(items) => Json(items).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub async fn get(Path(id): Path<String>) -> Response {
    match store::get(&id) {
        Ok(task) => Json(task).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "no such task").into_response(),
    }
}

#[derive(Deserialize)]
pub struct UpdateReq {
    title: String,
    body: String,
}

pub async fn update(Path(id): Path<String>, Json(req): Json<UpdateReq>) -> Response {
    match store::update(&id, &req.title, &req.body) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

pub async fn accept(State(swarm): State<Arc<Swarm>>, Path(id): Path<String>) -> Response {
    match swarm.accept(&id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub async fn reject(State(swarm): State<Arc<Swarm>>, Path(id): Path<String>) -> Response {
    match swarm.reject(&id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}
