use std::sync::Arc;

use clap::Parser;

mod agent;
mod app;
mod cli;
mod commands;
mod config;
mod context;
mod daemon;
mod llm;
mod runner;
mod server;
mod session;
mod skills;
mod swarm;
mod tools;
mod ui;
mod workflow;

use cli::{Args, BackendKind, Command};
use config::{AxonConfig, ModelEntry};
use llm::{Backend, daemon::DaemonBackend, ollama::OllamaBackend};

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    let args = Args::parse();
    let mut config = AxonConfig::load();

    // Subcommands that don't need a backend.
    if let Some(cmd) = &args.command {
        match cmd {
            Command::Stop => return commands::stop(),
            Command::Status => return commands::status().await,
            Command::Config(cmd) => return handle_config(cmd, &mut config),
            Command::Model(cmd) => return handle_model(cmd, &mut config),
            Command::Workflow(_) => {} // needs a backend — handled below
            Command::Serve(_) => {}    // needs model/url — handled below
        }
    }

    // Resolve effective values: CLI flag → config → platform default → ultimate fallback.
    let default_model = match std::env::consts::OS {
        "linux" => "qwen3.5:2b",
        "macos" => "qwen3.5:4b-mlx",
        _ => "qwen3.5:2b",
    };
    let model = args
        .model
        .as_deref()
        .or(config.model.as_deref())
        .unwrap_or(default_model)
        .to_string();
    let backend_kind = args.backend.clone().unwrap_or(BackendKind::Local);
    let ollama_url = args
        .ollama_url
        .as_deref()
        .or(config.ollama_url.as_deref())
        .unwrap_or("http://localhost:11434")
        .to_string();
    let no_download = args.no_download || config.no_download.unwrap_or(false);

    // Swarm dashboard: builds its own AutoAgents (Ollama) provider, not the
    // legacy Backend, so handle it before any daemon/backend setup.
    if let Some(Command::Serve(cmd)) = &args.command {
        let swarm = swarm::Swarm::new(&model, &ollama_url).await?;
        return server::run_server(swarm, cmd.host.clone(), cmd.port).await;
    }

    if args.daemon {
        return daemon::run_daemon(&model, no_download, args.context_window).await;
    }

    let backend: Arc<dyn Backend> = match &backend_kind {
        BackendKind::Local => {
            let port =
                daemon::ensure::ensure_daemon_running(&model, no_download, args.context_window)
                    .await?;
            let cw = args
                .context_window
                .unwrap_or_else(|| llm::local::resolve_cw(&model));
            Arc::new(DaemonBackend::new(port, &model, cw))
        }
        BackendKind::Ollama => {
            let num_ctx = args.context_window.or(config.context_window);
            Arc::new(OllamaBackend::new(&ollama_url, &model, num_ctx))
        }
    };

    // Workflow subcommand (needs backend).
    if let Some(Command::Workflow(cmd)) = &args.command {
        match &cmd.action {
            cli::WorkflowAction::Run { file, compile_only } => {
                let engine = workflow::WorkflowEngine::new(backend);
                return engine.run_workflow(file, *compile_only).await;
            }
        }
    }

    // Resolve skill (download if URL, find locally if name).
    let skill_content: Option<String> = if let Some(ref s) = args.skill {
        let skill = skills::resolve_skill(s)?;
        eprintln!("Using skill: {} — {}", skill.name, skill.description);
        Some(skill.content)
    } else {
        None
    };

    match args.prompt.clone() {
        Some(p) => runner::run_once(p, backend, args.context_window, skill_content).await,
        None => {
            app::run_tui(
                backend,
                backend_kind,
                ollama_url,
                no_download,
                args.context_window,
                skill_content,
            )
            .await
        }
    }
}

fn handle_config(cmd: &cli::ConfigCmd, config: &mut AxonConfig) -> color_eyre::Result<()> {
    match &cmd.action {
        cli::ConfigAction::Set { key, value } => {
            config.set(key, value)?;
            config.save()?;
            println!("Set {key} = {value}");
        }
        cli::ConfigAction::Get { key } => {
            let value = match key.as_str() {
                "model" => config.model.as_deref().unwrap_or("(not set)").to_string(),
                "backend" => config.backend.as_deref().unwrap_or("(not set)").to_string(),
                "ollama-url" => config
                    .ollama_url
                    .as_deref()
                    .unwrap_or("(not set)")
                    .to_string(),
                "context-window" => config
                    .context_window
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "(not set)".to_string()),
                "no-download" => config
                    .no_download
                    .map(|b| b.to_string())
                    .unwrap_or_else(|| "(not set)".to_string()),
                _ => color_eyre::eyre::bail!("unknown key '{key}'"),
            };
            println!("{value}");
        }
        cli::ConfigAction::List => {
            let path = AxonConfig::path()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "~/.axon/config.toml".to_string());
            println!("# {path}");
            if let Some(v) = &config.model {
                println!("model = {v}");
            }
            if let Some(v) = &config.backend {
                println!("backend = {v}");
            }
            if let Some(v) = &config.ollama_url {
                println!("ollama-url = {v}");
            }
            if let Some(v) = config.context_window {
                println!("context-window = {v}");
            }
            if let Some(v) = config.no_download {
                println!("no-download = {v}");
            }
            if !config.models.is_empty() {
                println!("\nRegistered models:");
                for m in &config.models {
                    println!("  {} ({} ctx tokens)", m.name, m.context_window);
                    println!("    repo: {}", m.hf_repo);
                    if !m.hf_file.is_empty() {
                        println!("    file: {}", m.hf_file);
                    }
                }
            }
        }
    }
    Ok(())
}

fn handle_model(cmd: &cli::ModelCmd, config: &mut AxonConfig) -> color_eyre::Result<()> {
    match &cmd.action {
        cli::ModelAction::Add {
            name,
            repo,
            file,
            context_window,
        } => {
            if let Some(existing) = config.models.iter_mut().find(|m| m.name == *name) {
                existing.hf_repo = repo.clone();
                existing.hf_file = file.clone();
                existing.context_window = *context_window;
                println!("Updated model '{name}'");
            } else {
                config.models.push(ModelEntry {
                    name: name.clone(),
                    hf_repo: repo.clone(),
                    hf_file: file.clone(),
                    context_window: *context_window,
                });
                println!("Registered model '{name}'");
            }
            config.save()?;
        }
        cli::ModelAction::Remove { name } => {
            let before = config.models.len();
            config.models.retain(|m| m.name != *name);
            if config.models.len() < before {
                config.save()?;
                println!("Removed model '{name}'");
            } else {
                println!("Model '{name}' not found in user registry");
            }
        }
        cli::ModelAction::List => {
            println!("Built-in models:");
            println!("  qwen3.5:2b   32768 ctx  ~1.3 GB");
            println!("    repo: unsloth/Qwen3.5-2B-GGUF");
            println!("    file: Qwen3.5-2B-Q4_K_M.gguf");
            println!();
            println!("  qwen3.5:4b-mlx   32768 ctx  ~2.7 GB  (recommended on macOS)");
            println!("    repo: unsloth/Qwen3.5-4B-GGUF");
            println!("    file: Qwen3.5-4B-Q4_K_M.gguf");
            println!();
            println!("  qwen3:4b   32768 ctx  ~2.5 GB");
            println!("    repo: unsloth/Qwen3-4B-GGUF");
            println!("    file: Qwen3-4B-Q4_K_M.gguf");
            println!();
            println!("To use Ollama instead:");
            println!("  axon -b ollama -m <model> \"hello\"");
            println!("  axon config set backend ollama");
            println!("  axon config set model <model>");
            if !config.models.is_empty() {
                println!("\nUser-defined models:");
                for m in &config.models {
                    println!("  {} ({} ctx tokens)", m.name, m.context_window);
                    println!("    repo: {}", m.hf_repo);
                    if !m.hf_file.is_empty() {
                        println!("    file: {}", m.hf_file);
                    }
                }
            }
        }
    }
    Ok(())
}
