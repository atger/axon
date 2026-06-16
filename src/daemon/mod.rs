pub mod ensure;
pub mod proto;

use std::path::PathBuf;
use std::sync::Arc;

use color_eyre::eyre::WrapErr;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, mpsc};
use tokio_util::sync::CancellationToken;

use crate::llm::{Backend, StreamEvent, local::LocalBackend};
use proto::{DaemonRequest, DaemonResponse};

pub fn axon_data_dir() -> color_eyre::Result<PathBuf> {
    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .wrap_err("HOME environment variable not set")?;
    Ok(home.join(".local").join("share").join("axon"))
}

pub fn axon_port_file() -> color_eyre::Result<PathBuf> {
    Ok(axon_data_dir()?.join("port"))
}

/// PID of the running daemon. Written immediately at startup (before model load)
/// so the CLI can detect a loading daemon and avoid spawning a second one.
pub fn axon_pid_file() -> color_eyre::Result<PathBuf> {
    Ok(axon_data_dir()?.join("daemon.pid"))
}

pub fn axon_log_file() -> color_eyre::Result<PathBuf> {
    Ok(axon_data_dir()?.join("daemon.log"))
}

pub async fn run_daemon(
    model: &str,
    no_download: bool,
    _cw: Option<usize>,
) -> color_eyre::Result<()> {
    // Create data dir and write PID file immediately — before model load.
    // The CLI checks this file to avoid spawning a second daemon while we're loading.
    std::fs::create_dir_all(axon_data_dir()?).wrap_err("failed to create data directory")?;
    std::fs::write(axon_pid_file()?, std::process::id().to_string())
        .wrap_err("failed to write PID file")?;

    // Load model before binding the port — the port file appearing is the readiness signal.
    let local = LocalBackend::new(model, no_download);
    eprintln!("axon-daemon: loading model {model}…");
    local.warm_up().await.wrap_err("failed to load model")?;
    eprintln!("axon-daemon: model loaded");

    // Race-condition guard: another daemon may have started while we were loading.
    let port_file = axon_port_file()?;
    if let Ok(s) = std::fs::read_to_string(&port_file)
        && let Ok(p) = s.trim().parse::<u16>()
        && TcpStream::connect(("127.0.0.1", p)).await.is_ok()
    {
        eprintln!("axon-daemon: another daemon already running on port {p}, exiting");
        return Ok(());
    }

    // Bind on port 0 so the OS assigns a free port (no conflicts).
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .wrap_err("failed to bind daemon port")?;
    let port = listener.local_addr()?.port();

    // Write port file — this unblocks any CLI polling in ensure_daemon_running.
    std::fs::write(&port_file, port.to_string()).wrap_err("failed to write port file")?;
    eprintln!("axon-daemon: listening on 127.0.0.1:{port}");

    let backend: Arc<dyn Backend> = Arc::new(local);
    // Semaphore(1): serializes inference — concurrent clients queue rather than error.
    let sem = Arc::new(Semaphore::new(1));

    loop {
        let (conn, _) = listener.accept().await.wrap_err("accept failed")?;
        let backend = Arc::clone(&backend);
        let sem = Arc::clone(&sem);
        let model = model.to_string();
        tokio::spawn(async move {
            let _permit = sem.acquire_owned().await.expect("semaphore closed");
            if let Err(e) = handle_connection(conn, backend, &model).await {
                eprintln!("axon-daemon: connection error: {e:#}");
            }
        });
    }
}

async fn handle_connection(
    conn: TcpStream,
    backend: Arc<dyn Backend>,
    daemon_model: &str,
) -> color_eyre::Result<()> {
    let (read_half, mut write_half) = conn.into_split();
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();

    if reader.read_line(&mut line).await? == 0 {
        return Ok(());
    }

    let req: DaemonRequest = match serde_json::from_str(line.trim()) {
        Ok(r) => r,
        Err(e) => {
            let resp = DaemonResponse {
                delta: None,
                done: true,
                error: Some(format!("invalid request JSON: {e}")),
            };
            let _ = write_half
                .write_all((serde_json::to_string(&resp)? + "\n").as_bytes())
                .await;
            return Ok(());
        }
    };

    if req.model_name != daemon_model {
        let resp = DaemonResponse {
            delta: None,
            done: true,
            error: Some(format!(
                "daemon serves '{}', client requested '{}'",
                daemon_model, req.model_name
            )),
        };
        write_half
            .write_all((serde_json::to_string(&resp)? + "\n").as_bytes())
            .await?;
        return Ok(());
    }

    let cancel = CancellationToken::new();
    let cancel2 = cancel.clone();
    let (tx, mut rx) = mpsc::channel::<StreamEvent>(64);

    let messages = req.messages;
    tokio::spawn(async move {
        let _ = backend.stream(&messages, cancel2, tx).await;
    });

    while let Some(ev) = rx.recv().await {
        let resp = DaemonResponse {
            delta: if ev.delta.is_empty() {
                None
            } else {
                Some(ev.delta)
            },
            done: ev.done,
            error: None,
        };
        if write_half
            .write_all((serde_json::to_string(&resp)? + "\n").as_bytes())
            .await
            .is_err()
        {
            // Client disconnected — cancel the in-flight inference.
            cancel.cancel();
            break;
        }
        if ev.done {
            break;
        }
    }

    Ok(())
}
