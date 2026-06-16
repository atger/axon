use serde::{Deserialize, Serialize};

use crate::session::Message;

/// Sent by the CLI to the daemon — one JSON line per request.
#[derive(Serialize, Deserialize)]
pub struct DaemonRequest {
    pub messages: Vec<Message>,
    /// Included so the daemon can detect a model mismatch (stale connection).
    pub model_name: String,
    /// Optional GBNF grammar for constrained sampling. Absent means unconstrained.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grammar: Option<String>,
}

/// Sent by the daemon to the CLI — one JSON line per streaming token.
#[derive(Serialize, Deserialize)]
pub struct DaemonResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta: Option<String>,
    pub done: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
