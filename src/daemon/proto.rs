use serde::{Deserialize, Serialize};

use crate::session::Message;

/// Sent by the CLI to the daemon — one JSON line per request.
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DaemonRequest {
    /// Run inference over the given message history.
    Infer { messages: Vec<Message> },
    /// Unload the current model and load a new one in-process.
    SwitchModel { model: String, no_download: bool },
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
