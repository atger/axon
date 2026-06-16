use std::path::PathBuf;
use std::time::{Duration, Instant};

use color_eyre::eyre::{self, WrapErr};
use tokio::net::TcpStream;

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
        return Ok(port);
    }

    // Check whether a daemon is already loading the model (PID file present + alive).
    // If so, skip spawning a second one — just wait for the port file to appear.
    let already_loading = try_read_pid(&pid_file).is_some_and(pid_is_alive);
    if !already_loading {
        let _ = std::fs::remove_file(&port_file);
        let _ = std::fs::remove_file(&pid_file);
        spawn_daemon(model, no_download, cw)?;
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
    let deadline = Instant::now() + Duration::from_secs(600);
    let mut last_progress = Instant::now();

    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;

        if let Some(port) = try_read_port(&port_file)
            && TcpStream::connect(("127.0.0.1", port)).await.is_ok()
        {
            return Ok(port);
        }

        if last_progress.elapsed() >= Duration::from_secs(15) {
            eprintln!("axon: still loading model…");
            last_progress = Instant::now();
        }

        if Instant::now() >= deadline {
            // Kill the daemon and clean up so the next run can start fresh.
            invalidate_daemon()?;
            eyre::bail!(
                "axon daemon failed to start within 10 minutes — check {}",
                axon_log_file()?.display()
            );
        }
    }
}
