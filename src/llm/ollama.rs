use async_trait::async_trait;
use futures::StreamExt;
use reqwest::Client;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{Backend, BackendError, InferOptions, StreamEvent};
use crate::session::{Message, Role};

pub struct OllamaBackend {
    base_url: String,
    model: String,
    client: Client,
}

impl OllamaBackend {
    pub fn new(base_url: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            model: model.into(),
            client: Client::new(),
        }
    }
}

fn msg_to_json(msg: &Message) -> Value {
    let role = match msg.role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
    };
    json!({ "role": role, "content": msg.content })
}

#[async_trait]
impl Backend for OllamaBackend {
    async fn stream(
        &self,
        messages: &[Message],
        _options: &InferOptions,
        cancel: CancellationToken,
        tx: mpsc::Sender<StreamEvent>,
    ) -> Result<(), BackendError> {
        let body = json!({
            "model": self.model,
            "messages": messages.iter().map(msg_to_json).collect::<Vec<_>>(),
            "stream": true,
        });

        let resp = self
            .client
            .post(format!("{}/api/chat", self.base_url))
            .json(&body)
            .send()
            .await
            .map_err(|e| BackendError::Unavailable(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(BackendError::Unavailable(format!("HTTP {}", resp.status())));
        }

        let mut stream = resp.bytes_stream();
        let mut buf = String::new();

        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                chunk = stream.next() => {
                    let Some(chunk) = chunk else { break };
                    let chunk = chunk.map_err(|e| BackendError::Inference(e.to_string()))?;
                    buf.push_str(&String::from_utf8_lossy(&chunk));

                    while let Some(pos) = buf.find('\n') {
                        let line = buf[..pos].trim().to_string();
                        buf = buf[pos + 1..].to_string();
                        if line.is_empty() {
                            continue;
                        }
                        if let Ok(val) = serde_json::from_str::<Value>(&line) {
                            let delta = val["message"]["content"]
                                .as_str()
                                .unwrap_or("")
                                .to_string();
                            let done = val["done"].as_bool().unwrap_or(false);
                            let _ = tx.send(StreamEvent { delta, done }).await;
                            if done {
                                return Ok(());
                            }
                        }
                    }
                }
            }
        }

        let _ = tx
            .send(StreamEvent {
                delta: String::new(),
                done: true,
            })
            .await;
        Ok(())
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    fn context_window(&self) -> usize {
        4096
    }
}
