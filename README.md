# axon

A terminal-based AI coding assistant built in Rust, designed to run entirely on local language models — including models as small as 1 billion parameters.

No cloud dependency. No API keys. Your code stays on your machine.

## Vision

Most AI coding tools require large cloud-hosted models and a constant internet connection. Axon flips that assumption: it is built from the ground up to work well on small, locally-run models. A 1B parameter model running on a CPU should give you a useful, responsive coding assistant. Larger models (3B, 7B, 13B+) give better results but are never required.

## Features (planned)

- **Local-first inference** — integrates with local model runtimes (llama.cpp, Ollama, candle)
- **Terminal UI** — keyboard-driven interface built with [Ratatui](https://ratatui.rs)
- **Context-aware** — reads your project files, git history, and diagnostics to ground responses
- **Streaming output** — tokens appear as they are generated, no waiting for full responses
- **Model-size aware** — prompt construction adapts to available context window (small models get tighter, focused prompts)
- **Offline capable** — fully functional without any network access once models are downloaded
- **Multi-model** — switch between models mid-session without restarting
- **MCP support** — extensible tool system via Model Context Protocol; integrates with GitHub, Google, and more.

## Tech Stack

| Layer | Choice |
|---|---|
| Language | Rust |
| Terminal UI | [Ratatui](https://github.com/ratatui-org/ratatui) |
| Local inference | llama.cpp / Ollama (via HTTP) |
| Async runtime | Tokio |

## Getting Started

> The project is in early development. These instructions will be updated as the build stabilizes.

**Prerequisites**

- Rust 1.78+ (`rustup update stable`)
- A local model runtime: [Ollama](https://ollama.com) (easiest) or a llama.cpp server

**Build**

```sh
git clone https://github.com/yourusername/axon
cd axon
cargo build --release
```

**Run**

```sh
# With Ollama running a small model
ollama pull qwen2.5-coder:1.5b
./target/release/axon
```

## Model Recommendations

Axon is tested against models in the 1B–7B range. Recommended starting points:

| Size | Model | Notes |
|---|---|---|
| 1–2B | `qwen2.5-coder:1.5b` | Minimum viable, fast on CPU |
| 3B | `qwen2.5-coder:3b` | Good balance on 8GB RAM |
| 7B | `qwen2.5-coder:7b` | Recommended with a GPU |

## Architecture

```
axon/
├── src/
│   ├── main.rs          # Entry point, runtime setup
│   ├── app.rs           # Top-level application state
│   ├── ui/              # Ratatui widgets and layout
│   ├── llm/             # Model backend abstraction + adapters
│   ├── context/         # File reading, git, diagnostics
│   └── session/         # Conversation history, prompt assembly
```

The `llm` module defines a backend trait so multiple inference runtimes can be swapped without touching the UI or context layers.

## Contributing

Contributions are welcome. A few things to keep in mind:

- Changes that break compatibility with 1B models are not accepted
- The UI must remain usable over SSH on an 80-column terminal
- No runtime dependencies on cloud services — the binary must work fully offline

## Model Context Protocol (MCP)

Axon supports [MCP](https://modelcontextprotocol.io/), allowing it to use a wide variety of external tools. By default, it includes configurations for GitHub and Google Search.

To configure MCP servers, edit `~/.axon/config.toml`:

```toml
[mcp_servers]
github = { command = "npx", args = ["-y", "@modelcontextprotocol/server-github"] }
google = { command = "npx", args = ["-y", "@modelcontextprotocol/server-google-search"] }
```

> **Note:** Many MCP servers require environment variables for authentication (e.g., `GITHUB_PERSONAL_ACCESS_TOKEN`). Set these in your shell before running Axon.

## License

Apache 2.0 — see [LICENSE](LICENSE).
