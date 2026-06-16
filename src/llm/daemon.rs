use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{Backend, BackendError, StreamEvent};
use crate::daemon::proto::{DaemonRequest, DaemonResponse};
use crate::session::Message;

pub struct DaemonBackend {
    port: u16,
    name: String,
    cw: usize,
}

impl DaemonBackend {
    pub fn new(port: u16, model_name: impl Into<String>, cw: usize) -> Self {
        Self {
            port,
            name: model_name.into(),
            cw,
        }
    }
}

#[async_trait]
impl Backend for DaemonBackend {
    async fn stream(
        &self,
        messages: &[Message],
        cancel: CancellationToken,
        tx: mpsc::Sender<StreamEvent>,
    ) -> Result<(), BackendError> {
        let conn = TcpStream::connect(("127.0.0.1", self.port))
            .await
            .map_err(|e| BackendError::Unavailable(format!("cannot connect to daemon: {e}")))?;

        let (read_half, mut write_half) = conn.into_split();

        let req = DaemonRequest {
            messages: messages.to_vec(),
            model_name: self.name.clone(),
        };
        let mut req_line =
            serde_json::to_string(&req).map_err(|e| BackendError::Inference(e.to_string()))?;
        req_line.push('\n');

        write_half
            .write_all(req_line.as_bytes())
            .await
            .map_err(|e| BackendError::Unavailable(format!("failed to send request: {e}")))?;

        let mut reader = BufReader::new(read_half);
        let mut buf = String::new();

        loop {
            buf.clear();
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    // Dropping write_half (and reader) closes the TCP connection.
                    // The daemon's next write will fail with broken pipe, which
                    // triggers cancel.cancel() on the daemon side → inference stops.
                    let _ = tx.send(StreamEvent { delta: String::new(), done: true }).await;
                    break;
                }
                result = reader.read_line(&mut buf) => {
                    let n = result.map_err(|e| BackendError::Inference(e.to_string()))?;
                    if n == 0 {
                        let _ = tx.send(StreamEvent { delta: String::new(), done: true }).await;
                        break;
                    }
                    let resp: DaemonResponse = serde_json::from_str(buf.trim())
                        .map_err(|e| BackendError::Inference(format!("bad daemon response: {e}")))?;
                    if let Some(err) = resp.error {
                        return Err(BackendError::Inference(err));
                    }
                    let _ = tx.send(StreamEvent {
                        delta: resp.delta.unwrap_or_default(),
                        done: resp.done,
                    }).await;
                    if resp.done {
                        break;
                    }
                }
            }
        }
        // write_half and reader dropped here — connection closed.
        Ok(())
    }

    fn model_name(&self) -> &str {
        &self.name
    }

    fn context_window(&self) -> usize {
        self.cw
    }
}
