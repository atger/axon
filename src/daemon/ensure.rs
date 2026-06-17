use std::path::PathBuf;
use std::time::{Duration, Instant};

use color_eyre::eyre::{self, WrapErr};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

use super::proto::{DaemonRequest, DaemonResponse};
use super::{axon_log_file, axon_model_file, axon_pid_file, axon_port_file};

pub(crate) fn try_read_port(port_file: &PathBuf) -> Option<u16> {
    std::fs::read_to_string(port_file)
        .ok()
        .and_then(|s| s.trim().parse::<u16>().ok())
}

pub(crate) fn try_read_pid(pid_file: &PathBuf) -> Option<u32> {
    std::fs::read_to_string(pid_file)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
}

/// Returns true if a process with the given PID is still alive.
/// Uses `kill -0` on Unix (signal 0 = existence check, no actual signal sent).
pub(crate) fn pid_is_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

/// Downloads the model file to the HuggingFace cache in the CLI process so
/// that the progress bar is visible on the TTY and errors surface immediately.
/// No-op if the file is already cached, the model has no HF file, or
/// `no_download` is set.
async fn ensure_model_downloaded(name: &str, no_download: bool) -> color_eyre::Result<()> {
    let (hf_repo, hf_file, _) = crate::llm::local::resolve_model(name);
    if hf_file.is_empty() || no_download {
        return Ok(());
    }
    tokio::task::spawn_blocking(move || {
        let api = hf_hub::api::sync::ApiBuilder::new()
            .with_progress(true)
            .build()
            .map_err(|e| color_eyre::eyre::eyre!("HuggingFace API error: {e}"))?;
        api.model(hf_repo)
            .get(&hf_file)
            .map_err(|e| color_eyre::eyre::eyre!("failed to download model: {e}"))?;
        Ok::<(), color_eyre::eyre::Error>(())
    })
    .await
    .wrap_err("download task panicked")??;
    Ok(())
}

fn spawn_daemon(model: &str, no_download: bool, cw: Option<usize>) -> color_eyre::Result<()> {
    let exe = std::env::current_exe().wrap_err("failed to find current executable")?;
    let log = axon_log_file()?;

    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)
        .wrap_err("failed to open daemon log file")?;

    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("--daemon").arg("--model").arg(model);
    if no_download {
        cmd.arg("--no-download");
    }
    if let Some(n) = cw {
        cmd.arg("--context-window").arg(n.to_string());
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(log_file);
    cmd.spawn().wrap_err("failed to spawn axon daemon")?;
    Ok(())
}

/// Terminates the daemon and removes its state files so the next invocation
/// spawns a fresh daemon. Used by `/model` to switch models.
pub fn invalidate_daemon() -> color_eyre::Result<()> {
    let pid_file = axon_pid_file()?;
    let port_file = axon_port_file()?;

    // Send SIGTERM to the daemon so it releases the model from memory.
    #[cfg(unix)]
    if let Some(pid) = try_read_pid(&pid_file) {
        let _ = std::process::Command::new("kill")
            .arg(pid.to_string())
            .status();
    }

    let _ = std::fs::remove_file(&port_file);
    let _ = std::fs::remove_file(&pid_file);
    let _ = std::fs::remove_file(axon_model_file().unwrap_or_default());
    Ok(())
}

/// Sends a `SwitchModel` request to the running daemon and waits for the ack.
/// This blocks until the new model is fully loaded (may include downloading).
async fn switch_model_request(port: u16, model: &str, no_download: bool) -> color_eyre::Result<()> {
    eprintln!("axon: switching to model '{model}'…");
    let conn = TcpStream::connect(("127.0.0.1", port))
        .await
        .wrap_err("cannot connect to daemon for model switch")?;
    let (read_half, mut write_half) = conn.into_split();

    let req = DaemonRequest::SwitchModel {
        model: model.to_string(),
        no_download,
    };
    let mut line = serde_json::to_string(&req).wrap_err("failed to serialize switch request")?;
    line.push('\n');
    write_half
        .write_all(line.as_bytes())
        .await
        .wrap_err("failed to send switch request")?;

    let mut reader = BufReader::new(read_half);
    let mut buf = String::new();
    reader
        .read_line(&mut buf)
        .await
        .wrap_err("failed to read switch response")?;
    let resp: DaemonResponse =
        serde_json::from_str(buf.trim()).wrap_err("bad response to switch request")?;
    if let Some(err) = resp.error {
        eyre::bail!("model switch failed: {err}");
    }
    Ok(())
}

/// Connects to the running daemon and returns its port, or spawns a new daemon
/// and waits up to 10 minutes for it to become ready (model loading can be slow,
/// especially in debug builds).
pub async fn ensure_daemon_running(
    model: &str,
    no_download: bool,
    cw: Option<usize>,
) -> color_eyre::Result<u16> {
    let port_file = axon_port_file()?;
    let pid_file = axon_pid_file()?;

    // Fast path: daemon is already up and connectable.
    if let Some(port) = try_read_port(&port_file)
        && TcpStream::connect(("127.0.0.1", port)).await.is_ok()
    {
        // Check whether it's serving the right model; hot-swap if not.
        let running = std::fs::read_to_string(axon_model_file()?).unwrap_or_default();
        if running.trim() == model {
            return Ok(port);
        }
        ensure_model_downloaded(model, no_download).await?;
        match switch_model_request(port, model, true).await {
            Ok(()) => return Ok(port),
            Err(e) if e.to_string().contains("invalid request JSON") => {
                // The running daemon was built with an older binary and doesn't
                // understand the SwitchModel protocol — kill it and respawn.
                eprintln!("axon: daemon protocol mismatch, restarting…");
                invalidate_daemon()?;
                // fall through to the spawn path below
            }
            Err(e) => return Err(e),
        }
    }

    // Always download first (shows progress bar on TTY, errors early on failure).
    // This is a no-op if the file is already cached or no_download is set.
    ensure_model_downloaded(model, no_download).await?;

    // Check whether a daemon is already loading the model (PID file present + alive).
    // If so, skip spawning a second one — just wait for the port file to appear.
    let already_loading = try_read_pid(&pid_file).is_some_and(pid_is_alive);
    if !already_loading {
        let _ = std::fs::remove_file(&port_file);
        let _ = std::fs::remove_file(&pid_file);
        spawn_daemon(model, true, cw)?;
        eprintln!(
            "axon: daemon spawned — loading model '{model}' (log: {})",
            axon_log_file()
                .as_deref()
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        );
    } else {
        eprintln!("axon: daemon is loading model '{model}', waiting…");
    }

    // Poll until the port file appears and the daemon accepts connections.
    // Timeout is generous: debug builds + first HF download can take many minutes.
    let start = Instant::now();
    let deadline = start + Duration::from_secs(600);
    let is_tty = std::io::IsTerminal::is_terminal(&std::io::stderr());
    let mut last_secs = u64::MAX;

    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;

        if let Some(port) = try_read_port(&port_file)
            && TcpStream::connect(("127.0.0.1", port)).await.is_ok()
        {
            if is_tty {
                eprintln!();
            }
            return Ok(port);
        }

        // Detect daemon death early rather than waiting for the full 10-min timeout.
        if try_read_pid(&pid_file).is_none_or(|pid| !pid_is_alive(pid)) {
            // Clean up stale files so the next invocation starts fresh automatically.
            if is_tty {
                eprintln!();
            }
            let _ = std::fs::remove_file(&port_file);
            let _ = std::fs::remove_file(&pid_file);
            let _ = std::fs::remove_file(axon_model_file().unwrap_or_default());
            let log_tail = std::fs::read_to_string(axon_log_file()?).unwrap_or_default();
            let tail: String = log_tail
                .lines()
                .rev()
                .take(10)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n");
            eyre::bail!("axon daemon exited unexpectedly:\n{tail}");
        }

        let elapsed = start.elapsed().as_secs();
        if elapsed != last_secs {
            last_secs = elapsed;
            let (m, s) = (elapsed / 60, elapsed % 60);
            let progress = if m > 0 {
                format!("{m}m {s}s")
            } else {
                format!("{s}s")
            };
            if is_tty {
                eprint!("\raxon: loading '{model}'… {progress}");
            } else if elapsed > 0 && elapsed.is_multiple_of(15) {
                eprintln!("axon: loading '{model}'… {progress}");
            }
        }

        if Instant::now() >= deadline {
            if is_tty {
                eprintln!();
            }
            // Kill the daemon and clean up so the next run can start fresh.
            invalidate_daemon()?;
            eyre::bail!(
                "axon daemon failed to start within 10 minutes — check {}",
                axon_log_file()?.display()
            );
        }
    }
}
