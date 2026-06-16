use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::session::Message;

pub mod daemon;
pub mod local;
pub mod ollama;

#[derive(Debug, Clone)]
pub struct StreamEvent {
    pub delta: String,
    pub done: bool,
}

/// Per-request inference options passed through all backend implementations.
#[derive(Default, Clone)]
pub struct InferOptions {
    /// GBNF grammar string; when set, the local backend constrains sampling to
    /// tokens that keep the output valid according to the grammar.
    pub grammar: Option<String>,
}

#[derive(thiserror::Error, Debug)]
pub enum BackendError {
    #[allow(dead_code)]
    #[error("model not cached and --no-download is set")]
    ModelNotCached,
    #[error("backend unavailable: {0}")]
    Unavailable(String),
    #[error("inference error: {0}")]
    Inference(String),
}

#[async_trait]
pub trait Backend: Send + Sync {
    async fn stream(
        &self,
        messages: &[Message],
        options: &InferOptions,
        cancel: CancellationToken,
        tx: mpsc::Sender<StreamEvent>,
    ) -> Result<(), BackendError>;

    fn model_name(&self) -> &str;
    fn context_window(&self) -> usize;
}
