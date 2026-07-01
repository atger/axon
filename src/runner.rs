use std::io::Write;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::agent::AgentLoop;
use crate::context::ContextProvider;
use crate::llm::Backend;
use crate::session::{ConversationHistory, Message};
use crate::tools::ToolRegistry;

pub async fn run_once(
    prompt: String,
    backend: Arc<dyn Backend>,
    config: &crate::config::AxonConfig,
    context_window: Option<usize>,
    skill_content: Option<String>,
) -> color_eyre::Result<()> {
    let (tools, _clients): (_, Vec<_>) = ToolRegistry::from_config(config).await;
    let tools = Arc::new(tools);
    let ctx = ContextProvider::new(skill_content);
    let cw = context_window.unwrap_or_else(|| backend.context_window());

    let system_prompt = format!("{}\n{}", ctx.system_prompt(), tools.system_prompt_section());

    let mut history = ConversationHistory::new(cw);
    history.push(Message::user(prompt));
    let messages = history.assemble(&system_prompt);

    let (tx, mut rx) = mpsc::channel(64);
    let cancel = CancellationToken::new();

    let agent = AgentLoop::new(Arc::clone(&backend), Arc::clone(&tools));
    let confirm: crate::agent::ConfirmFn = Box::new(|tool_name, args_summary| {
        Box::pin(async move {
            eprint!("Run {tool_name} ({args_summary})? [y/N] ");
            let _ = std::io::stderr().flush();
            let mut line = String::new();
            std::io::stdin().read_line(&mut line).unwrap_or(0);
            matches!(line.trim().to_lowercase().as_str(), "y" | "yes")
        })
    });

    let cancel2 = cancel.clone();
    tokio::spawn(async move {
        if let Err(e) = agent.run(messages, cancel2, &confirm, tx).await {
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
