use clap::{Parser, Subcommand, ValueEnum};

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Stop the running daemon and free the model from memory.
    Stop,
    /// Show the status of the running daemon.
    Status,
}

#[derive(Parser, Debug)]
#[command(name = "axon", version, about = "Local AI coding assistant")]
pub struct Args {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Non-interactive prompt; streams to stdout then exits
    pub prompt: Option<String>,

    /// Model name
    #[arg(short = 'm', long, default_value = "qwen2.5-coder:1.5b")]
    pub model: String,

    /// Inference backend
    #[arg(short = 'b', long, value_enum, default_value = "local")]
    pub backend: BackendKind,

    /// Run as background daemon process (internal use)
    #[arg(long, hide = true)]
    pub daemon: bool,

    /// Ollama base URL
    #[arg(long, default_value = "http://localhost:11434")]
    pub ollama_url: String,

    /// Fail if model not cached; do not download
    #[arg(long)]
    pub no_download: bool,

    /// Override context window size (tokens)
    #[arg(long)]
    pub context_window: Option<usize>,
}

#[derive(ValueEnum, Clone, Debug)]
pub enum BackendKind {
    Local,
    Ollama,
}
