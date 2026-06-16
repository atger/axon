use std::sync::Arc;

use clap::Parser;

mod app;
mod cli;
mod context;
mod daemon;
mod llm;
mod runner;
mod session;
mod ui;

use cli::{Args, BackendKind};
use llm::{Backend, daemon::DaemonBackend, ollama::OllamaBackend};

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    let args = Args::parse();

    if args.daemon {
        return daemon::run_daemon(&args.model, args.no_download, args.context_window).await;
    }

    let backend: Arc<dyn Backend> = match args.backend {
        BackendKind::Local => {
            eprintln!("Starting axon daemon (model: {})…", args.model);
            let port = daemon::ensure::ensure_daemon_running(
                &args.model,
                args.no_download,
                args.context_window,
            )
            .await?;
            let cw = args
                .context_window
                .unwrap_or_else(|| llm::local::resolve_cw(&args.model));
            Arc::new(DaemonBackend::new(port, &args.model, cw))
        }
        BackendKind::Ollama => Arc::new(OllamaBackend::new(&args.ollama_url, &args.model)),
    };

    match args.prompt.clone() {
        Some(p) => runner::run_once(p, backend, &args).await,
        None => app::run_tui(backend, &args).await,
    }
}
