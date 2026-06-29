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
    /// Define and execute multi-step AI workflows.
    Workflow(WorkflowCmd),
    /// Run the web dashboard for the agent swarm.
    Serve(ServeCmd),
}

#[derive(clap::Args, Debug)]
pub struct ServeCmd {
    /// Address to bind. Use 0.0.0.0 to expose on the network (no auth — see warning).
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,
    /// Port to listen on.
    #[arg(long, default_value_t = 8420)]
    pub port: u16,
    /// Max agents decoding concurrently (real parallelism needs OLLAMA_NUM_PARALLEL).
    #[arg(long, default_value_t = 1)]
    pub max_concurrency: usize,
    /// Disable the built-in researcher→okf-writer→developer pipeline.
    #[arg(long)]
    pub no_research_agent: bool,
    /// Seconds between research cycles for the pipeline.
    #[arg(long, default_value_t = 300)]
    pub research_interval: u64,
    /// Max build-fix attempts the developer agent makes per accepted suggestion.
    #[arg(long, default_value_t = 5)]
    pub max_implement_attempts: usize,
}

#[derive(clap::Args, Debug)]
pub struct WorkflowCmd {
    #[command(subcommand)]
    pub action: WorkflowAction,
}

#[derive(Subcommand, Debug)]
pub enum WorkflowAction {
    /// Compile and execute a workflow markdown file.
    Run {
        /// Path to the workflow markdown file.
        file: std::path::PathBuf,
        /// Print the compiled JSON plan without executing.
        #[arg(long)]
        compile_only: bool,
    },
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

    /// Run with a skill by name (from ~/.axon/skills/) or GitHub URL.
    /// If a URL is given, the skill is downloaded and saved before the session starts.
    #[arg(long)]
    pub skill: Option<String>,
}

#[derive(ValueEnum, Clone, Debug)]
pub enum BackendKind {
    Local,
    Ollama,
}
