use std::io::Write;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::cli::Args;
use crate::context::ContextProvider;
use crate::llm::Backend;
use crate::session::{ConversationHistory, Message};

pub async fn run_once(
    prompt: String,
    backend: Arc<dyn Backend>,
    args: &Args,
) -> color_eyre::Result<()> {
    let ctx = ContextProvider::new();
    let cw = args
        .context_window
        .unwrap_or_else(|| backend.context_window());
    let mut history = ConversationHistory::new(cw);
    history.push(Message::user(prompt));

    let messages = history.assemble(&ctx.system_prompt());
    let (tx, mut rx) = mpsc::channel(64);
    let cancel = CancellationToken::new();

    let backend_clone = Arc::clone(&backend);
    let msgs = messages.clone();
    let cancel2 = cancel.clone();
    tokio::spawn(async move {
        if let Err(e) = backend_clone.stream(&msgs, cancel2, tx).await {
            eprintln!("\nError: {e}");
        }
    });

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    while let Some(event) = rx.recv().await {
        if !event.delta.is_empty() {
            write!(out, "{}", event.delta)?;
            out.flush()?;
        }
        if event.done {
            break;
        }
    }
    writeln!(out)?;
    Ok(())
}
