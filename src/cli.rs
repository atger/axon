use clap::{Parser, Subcommand, ValueEnum};

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Stop the running daemon and free the model from memory.
    Stop,
    /// Show the status of the running daemon.
    Status,
    /// Manage Axon configuration.
    Config(ConfigCmd),
    /// Manage registered models.
    Model(ModelCmd),
}

#[derive(clap::Args, Debug)]
pub struct ConfigCmd {
    #[command(subcommand)]
    pub action: ConfigAction,
}

#[derive(Subcommand, Debug)]
pub enum ConfigAction {
    /// Set a configuration value (e.g. axon config set model qwen3:1.7b).
    Set { key: String, value: String },
    /// Print a configuration value.
    Get { key: String },
    /// List all configuration values.
    List,
}

#[derive(clap::Args, Debug)]
pub struct ModelCmd {
    #[command(subcommand)]
    pub action: ModelAction,
}

#[derive(Subcommand, Debug)]
pub enum ModelAction {
    /// Register a HuggingFace GGUF model by alias.
    Add {
        name: String,
        #[arg(long)]
        repo: String,
        #[arg(long)]
        file: String,
        #[arg(long, default_value_t = 4096)]
        context_window: usize,
    },
    /// Remove a registered model by alias.
    Remove { name: String },
    /// List built-in and user-registered models.
    List,
}

#[derive(Parser, Debug)]
#[command(name = "axon", version, about = "Local AI coding assistant")]
pub struct Args {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Non-interactive prompt; streams to stdout then exits
    pub prompt: Option<String>,

    /// Model name (overrides config)
    #[arg(short = 'm', long)]
    pub model: Option<String>,

    /// Inference backend (overrides config)
    #[arg(short = 'b', long, value_enum)]
    pub backend: Option<BackendKind>,

    /// Run as background daemon process (internal use)
    #[arg(long, hide = true)]
    pub daemon: bool,

    /// Ollama base URL (overrides config)
    #[arg(long)]
    pub ollama_url: Option<String>,

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
