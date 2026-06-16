use tokio::net::TcpStream;

use crate::daemon::{
    axon_log_file, axon_model_file, axon_pid_file, axon_port_file,
    ensure::{invalidate_daemon, pid_is_alive, try_read_pid, try_read_port},
};

/// Stops the running daemon, freeing the model from memory.
pub fn stop() -> color_eyre::Result<()> {
    let pid_file = axon_pid_file()?;
    match try_read_pid(&pid_file) {
        None => {
            println!("No daemon is running.");
        }
        Some(pid) if !pid_is_alive(pid) => {
            println!("No daemon is running (stale pid file, cleaning up).");
            invalidate_daemon()?;
        }
        Some(pid) => {
            invalidate_daemon()?;
            println!("Stopped daemon (pid {pid}).");
        }
    }
    Ok(())
}

/// Prints the status of the running daemon.
pub async fn status() -> color_eyre::Result<()> {
    let pid_file = axon_pid_file()?;
    let port_file = axon_port_file()?;

    let Some(pid) = try_read_pid(&pid_file) else {
        println!("daemon:  not running");
        return Ok(());
    };

    if !pid_is_alive(pid) {
        println!("daemon:  dead (stale pid file — run `axon stop` to clean up)");
        return Ok(());
    }

    let port = try_read_port(&port_file);
    let model = std::fs::read_to_string(axon_model_file()?).unwrap_or_else(|_| "unknown".into());
    let log = axon_log_file()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "unknown".into());

    let conn_status = match port {
        Some(p) if TcpStream::connect(("127.0.0.1", p)).await.is_ok() => "accepting connections",
        Some(_) => "not responding",
        None => "loading model…",
    };

    println!("daemon:  running  (pid {pid})");
    match port {
        Some(p) => println!("port:    {p}"),
        None => println!("port:    (not yet bound — model loading)"),
    }
    println!("model:   {}", model.trim());
    println!("status:  {conn_status}");
    println!("log:     {log}");

    Ok(())
}
