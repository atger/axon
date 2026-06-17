use std::sync::Arc;

use futures::future::BoxFuture;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::llm::{Backend, BackendError, InferOptions, StreamEvent};
use crate::session::Message;
use crate::tools::ToolRegistry;

/// Async function that requests user confirmation for a destructive tool call.
/// Receives (tool_name, args_summary) and returns true if the user confirmed.
pub type ConfirmFn =
    Box<dyn Fn(String, String) -> BoxFuture<'static, bool> + Send + Sync + 'static>;

/// Drives the tool-calling loop: generates a response (expected to be one of two
/// JSON shapes), dispatches tool calls, and repeats until the model produces a
/// text response or the iteration cap is reached.
pub struct AgentLoop {
    backend: Arc<dyn Backend>,
    tools: Arc<ToolRegistry>,
}

impl AgentLoop {
    pub fn new(backend: Arc<dyn Backend>, tools: Arc<ToolRegistry>) -> Self {
        Self { backend, tools }
    }

    /// Runs the agentic loop.
    ///
    /// `text_tx` receives the final text response token-by-token (or all at
    /// once), terminated by a `StreamEvent { done: true }`.
    ///
    /// Tool calls are executed internally; `confirm` is called before any
    /// destructive tool and must resolve to `true` for execution to proceed.
    pub async fn run(
        &self,
        mut messages: Vec<Message>,
        cancel: CancellationToken,
        confirm: &ConfirmFn,
        text_tx: mpsc::Sender<StreamEvent>,
    ) -> Result<(), BackendError> {
        const MAX_ITER: usize = 8;
        let options = InferOptions::default();
        let mut nudged = false;

        for _ in 0..MAX_ITER {
            if cancel.is_cancelled() {
                break;
            }

            // Collect the full response into a buffer.
            let (stream_tx, mut stream_rx) = mpsc::channel::<StreamEvent>(256);
            let backend = Arc::clone(&self.backend);
            let msgs = messages.clone();
            let opts = options.clone();
            let cancel2 = cancel.clone();
            tokio::spawn(async move {
                let _ = backend.stream(&msgs, &opts, cancel2, stream_tx).await;
            });

            let mut buf = String::new();
            while let Some(ev) = stream_rx.recv().await {
                if cancel.is_cancelled() {
                    break;
                }
                if !ev.delta.is_empty() {
                    buf.push_str(&ev.delta);
                }
                if ev.done {
                    break;
                }
            }

            if cancel.is_cancelled() {
                break;
            }

            if std::env::var("AXON_TRACE").is_ok() {
                eprintln!("[trace] raw model output: {buf:?}");
            }

            // Strip <think>...</think> blocks (Qwen3 thinking mode) then find the
            // leading JSON object. Fall back to emitting raw output on parse failure.
            let response: serde_json::Value = match serde_json::from_str(extract_json(&buf)) {
                Ok(v) => v,
                Err(_) => {
                    if !nudged {
                        nudged = true;
                        if std::env::var("AXON_TRACE").is_ok() {
                            eprintln!("[trace] JSON parse failed — injecting nudge and retrying");
                        }
                        messages.push(Message::user(
                            "[System: output JSON only. Use web_search for current events or when you lack information.]"
                                .to_string(),
                        ));
                        continue;
                    }
                    let _ = text_tx
                        .send(StreamEvent {
                            delta: buf,
                            done: false,
                        })
                        .await;
                    break;
                }
            };

            match response["type"].as_str() {
                Some("text") => {
                    let content = response["content"].as_str().unwrap_or("").to_string();
                    let _ = text_tx
                        .send(StreamEvent {
                            delta: content,
                            done: false,
                        })
                        .await;
                    break;
                }

                Some("tool_call") => {
                    let name = response["name"].as_str().unwrap_or("").to_string();
                    let args = response["args"].clone();

                    if self.tools.needs_confirm(&name, &args) {
                        let args_summary = serde_json::to_string(&args).unwrap_or_default();
                        let confirmed = confirm(name.clone(), args_summary).await;
                        if !confirmed {
                            let _ = text_tx
                                .send(StreamEvent {
                                    delta: format!("Tool call `{name}` was not confirmed."),
                                    done: false,
                                })
                                .await;
                            break;
                        }
                    }

                    let result = match self.tools.execute(&name, args) {
                        Ok(r) => r,
                        Err(e) => format!("[tool error: {e}]"),
                    };

                    if std::env::var("AXON_TRACE").is_ok() {
                        eprintln!("[trace] tool result: {result:?}");
                    }

                    let today = chrono::Local::now().format("%Y-%m-%d");
                    messages.push(Message::user(format!(
                        "[Tool: {name} | today is {today}]\n{result}"
                    )));
                }

                _ => {
                    // Unexpected shape — emit raw output.
                    let _ = text_tx
                        .send(StreamEvent {
                            delta: buf,
                            done: false,
                        })
                        .await;
                    break;
                }
            }
        }

        let _ = text_tx
            .send(StreamEvent {
                delta: String::new(),
                done: true,
            })
            .await;
        Ok(())
    }
}

/// Strips `<think>...</think>` blocks and returns the slice starting from the
/// first `{`, so Qwen3 thinking tokens don't break JSON parsing.
fn extract_json(s: &str) -> &str {
    let s = if let (Some(start), Some(end_tag)) = (s.find("<think>"), s.find("</think>")) {
        let end = end_tag + "</think>".len();
        if start == 0 { s[end..].trim_start() } else { s }
    } else {
        s
    };
    let trimmed = s.trim();
    if let Some(brace) = trimmed.find('{') {
        trimmed[brace..].trim_end()
    } else {
        trimmed
    }
}
