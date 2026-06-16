use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Direction, Layout},
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::cli::{Args, BackendKind};
use crate::context::ContextProvider;
use crate::llm::{Backend, StreamEvent, daemon::DaemonBackend, ollama::OllamaBackend};
use crate::session::{ConversationHistory, Message};
use crate::ui::{
    chat::ChatWidget,
    input::InputWidget,
    status::{self, GenState},
};

enum AppEvent {
    Crossterm(Event),
    StreamDelta(String),
    StreamDone,
    StreamError(String),
}

enum Generating {
    Idle,
    Active {
        cancel: CancellationToken,
        partial: String,
    },
}

pub struct App<'a> {
    running: bool,
    session: ConversationHistory,
    backend: Arc<dyn Backend>,
    // Stored so /model can rebuild the backend without re-parsing CLI args.
    backend_kind: BackendKind,
    ollama_url: String,
    no_download: bool,
    context: ContextProvider,
    chat: ChatWidget,
    input: InputWidget<'a>,
    generating: Generating,
    /// True while waiting for the daemon to start (e.g. during /model switch).
    connecting: bool,
    /// User scrolled up manually; suppress auto-scroll-to-bottom while true.
    user_scrolled: bool,
}

impl<'a> App<'a> {
    fn new(backend: Arc<dyn Backend>, args: &Args) -> Self {
        let context = ContextProvider::new();
        let cw = args
            .context_window
            .unwrap_or_else(|| backend.context_window());
        Self {
            running: true,
            session: ConversationHistory::new(cw),
            backend,
            backend_kind: args.backend.clone(),
            ollama_url: args.ollama_url.clone(),
            no_download: args.no_download,
            context,
            chat: ChatWidget::new(),
            input: InputWidget::new(),
            generating: Generating::Idle,
            connecting: false,
            user_scrolled: false,
        }
    }

    async fn run(
        mut self,
        terminal: &mut DefaultTerminal,
        app_tx: mpsc::Sender<AppEvent>,
        mut app_rx: mpsc::Receiver<AppEvent>,
    ) -> color_eyre::Result<()> {
        // Spawn crossterm event reader into the app channel
        let tx2 = app_tx.clone();
        tokio::spawn(async move {
            loop {
                let ev = tokio::task::spawn_blocking(crossterm::event::read).await;
                match ev {
                    Ok(Ok(ev)) => {
                        if tx2.send(AppEvent::Crossterm(ev)).await.is_err() {
                            break;
                        }
                    }
                    _ => break,
                }
            }
        });

        while self.running {
            terminal.draw(|f| self.render(f))?;

            // Drive the tick with a timeout so the blinking cursor redraws
            let timeout = tokio::time::sleep(Duration::from_millis(100));
            tokio::select! {
                _ = timeout => {}
                Some(ev) = app_rx.recv() => {
                    self.handle_event(ev, &app_tx).await?;
                }
            }
        }
        Ok(())
    }

    fn render(&mut self, frame: &mut Frame) {
        let area = frame.area();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // status bar
                Constraint::Min(5),    // chat
                Constraint::Length(5), // input (min 3 + 2 borders)
            ])
            .split(area);

        let gen_state = if self.connecting {
            GenState::Connecting
        } else {
            match &self.generating {
                Generating::Idle => GenState::Idle,
                Generating::Active { .. } => GenState::Generating,
            }
        };
        status::render(
            frame,
            chunks[0],
            self.backend.model_name(),
            self.context.branch(),
            &gen_state,
        );

        let streaming = match &self.generating {
            Generating::Active { partial, .. } => Some(partial.as_str()),
            Generating::Idle => None,
        };
        // Collect displayed messages (exclude bare system prompt)
        let display_msgs: Vec<Message> = self
            .session
            .messages()
            .iter()
            .filter(|m| m.role != crate::session::Role::System)
            .cloned()
            .collect();
        self.chat.render(frame, chunks[1], &display_msgs, streaming);
        self.input.render(frame, chunks[2]);
    }

    async fn handle_event(
        &mut self,
        ev: AppEvent,
        app_tx: &mpsc::Sender<AppEvent>,
    ) -> color_eyre::Result<()> {
        match ev {
            AppEvent::Crossterm(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                self.on_key(key, app_tx).await?;
            }
            AppEvent::Crossterm(Event::Resize(..)) => {}
            AppEvent::Crossterm(_) => {}

            AppEvent::StreamDelta(delta) => {
                if let Generating::Active { partial, .. } = &mut self.generating {
                    partial.push_str(&delta);
                }
                if !self.user_scrolled {
                    self.chat.scroll_to_bottom();
                }
            }
            AppEvent::StreamDone => {
                if let Generating::Active { partial, .. } =
                    std::mem::replace(&mut self.generating, Generating::Idle)
                {
                    self.session.push(Message::assistant(partial));
                }
                self.user_scrolled = false;
                self.chat.scroll_to_bottom();
            }
            AppEvent::StreamError(e) => {
                if let Generating::Active { .. } =
                    std::mem::replace(&mut self.generating, Generating::Idle)
                {
                    self.session
                        .push(Message::assistant(format!("[error] {e}")));
                }
            }
        }
        Ok(())
    }

    async fn on_key(
        &mut self,
        key: KeyEvent,
        app_tx: &mpsc::Sender<AppEvent>,
    ) -> color_eyre::Result<()> {
        // Global shortcuts take precedence
        match (key.modifiers, key.code) {
            (KeyModifiers::CONTROL, KeyCode::Char('c') | KeyCode::Char('C')) => {
                match &self.generating {
                    Generating::Active { cancel, .. } => {
                        cancel.cancel();
                        // StreamDone will fire and finalize
                    }
                    Generating::Idle => self.running = false,
                }
                return Ok(());
            }
            (_, KeyCode::PageUp) => {
                self.user_scrolled = true;
                self.chat.scroll_up(10);
                return Ok(());
            }
            (_, KeyCode::PageDown) => {
                self.chat.scroll_down(10);
                if self.chat.scroll_offset == 0 {
                    self.user_scrolled = false;
                }
                return Ok(());
            }
            (_, KeyCode::Up) if matches!(self.generating, Generating::Idle) => {
                // Only navigate history when input is empty or on first line
                if self.input.text().trim().is_empty() || !self.input.text().contains('\n') {
                    self.input.history_up();
                    return Ok(());
                }
            }
            (_, KeyCode::Down) if matches!(self.generating, Generating::Idle) => {
                if self.input.text().trim().is_empty() || !self.input.text().contains('\n') {
                    self.input.history_down();
                    return Ok(());
                }
            }
            (_, KeyCode::Esc) => {
                self.input.clear();
                return Ok(());
            }
            (_, KeyCode::Enter) if key.modifiers != KeyModifiers::SHIFT => {
                self.handle_submit(app_tx).await?;
                return Ok(());
            }
            _ => {}
        }

        // Pass everything else to the textarea
        self.input.input(key);
        Ok(())
    }

    async fn handle_submit(&mut self, app_tx: &mpsc::Sender<AppEvent>) -> color_eyre::Result<()> {
        if matches!(self.generating, Generating::Active { .. }) {
            return Ok(());
        }

        let text = self.input.submit();
        let text = text.trim().to_string();
        if text.is_empty() {
            return Ok(());
        }

        // Handle built-in commands
        if text.starts_with('/') {
            self.handle_command(&text).await?;
            return Ok(());
        }

        // @file prefix — prepend file content
        let (user_text, file_prefix) = if let Some(path) = text.strip_prefix('@') {
            let path = path.split_whitespace().next().unwrap_or("");
            match std::fs::read_to_string(path) {
                Ok(contents) => {
                    let rest = text[1 + path.len()..].trim().to_string();
                    let combined = format!("File `{path}`:\n```\n{contents}\n```\n{rest}");
                    (combined, true)
                }
                Err(e) => {
                    self.session
                        .push(Message::assistant(format!("[error reading {path}] {e}")));
                    return Ok(());
                }
            }
        } else {
            (text.clone(), false)
        };
        let _ = file_prefix; // suppress unused warning

        self.session.push(Message::user(user_text));
        self.chat.scroll_to_bottom();

        let messages = self.session.assemble(&self.context.system_prompt());
        let (stream_tx, mut stream_rx) = mpsc::channel::<StreamEvent>(64);
        let cancel = CancellationToken::new();

        self.generating = Generating::Active {
            cancel: cancel.clone(),
            partial: String::new(),
        };

        let backend = Arc::clone(&self.backend);
        let app_tx2 = app_tx.clone();
        let cancel2 = cancel.clone();
        tokio::spawn(async move {
            match backend.stream(&messages, cancel2, stream_tx).await {
                Ok(()) => {}
                Err(e) => {
                    let _ = app_tx2.send(AppEvent::StreamError(e.to_string())).await;
                }
            }
        });

        // Forward stream events to the app channel
        let app_tx3 = app_tx.clone();
        tokio::spawn(async move {
            while let Some(ev) = stream_rx.recv().await {
                if ev.done {
                    let _ = app_tx3.send(AppEvent::StreamDone).await;
                    break;
                } else if !ev.delta.is_empty() {
                    let _ = app_tx3.send(AppEvent::StreamDelta(ev.delta)).await;
                }
            }
        });

        Ok(())
    }

    #[allow(clippy::unused_async)]
    async fn handle_command(&mut self, cmd: &str) -> color_eyre::Result<()> {
        let parts: Vec<&str> = cmd.splitn(2, ' ').collect();
        match parts[0] {
            "/model" => {
                if let Some(name) = parts.get(1).map(|s| s.trim()).filter(|s| !s.is_empty()) {
                    let new_backend: Arc<dyn Backend> = match self.backend_kind {
                        BackendKind::Ollama => Arc::new(OllamaBackend::new(&self.ollama_url, name)),
                        BackendKind::Local => {
                            self.connecting = true;
                            crate::daemon::ensure::invalidate_daemon()?;
                            // Brief wait so the old daemon detects the stale port file.
                            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                            match crate::daemon::ensure::ensure_daemon_running(
                                name,
                                self.no_download,
                                None,
                            )
                            .await
                            {
                                Ok(port) => {
                                    self.connecting = false;
                                    let cw = crate::llm::local::resolve_cw(name);
                                    Arc::new(DaemonBackend::new(port, name, cw))
                                }
                                Err(e) => {
                                    self.connecting = false;
                                    self.session.push(Message::assistant(format!(
                                        "[error starting daemon] {e}"
                                    )));
                                    return Ok(());
                                }
                            }
                        }
                    };
                    let cw = new_backend.context_window();
                    self.backend = new_backend;
                    self.session.set_context_window(cw);
                    self.session
                        .push(Message::assistant(format!("Switched to model `{name}`.")));
                } else {
                    self.session.push(Message::assistant(format!(
                        "Current model: `{}`\nUsage: /model <name>",
                        self.backend.model_name()
                    )));
                }
            }
            "/quit" | "/exit" => {
                self.running = false;
            }
            "/clear" => {
                self.session.clear();
                self.chat.scroll_to_bottom();
            }
            "/help" => {
                self.session.push(Message::assistant(
                    "/clear          — clear conversation\n\
                     /model [name]   — show or switch the active model\n\
                     /add <path>     — attach a file as context\n\
                     /help           — show this message\n\
                     /quit           — exit axon\n\
                     @<path> <msg>   — prefix your message with a file's contents\n\
                     Shift+Enter     — insert newline\n\
                     ↑ / ↓           — navigate input history\n\
                     PgUp / PgDn     — scroll chat\n\
                     Ctrl+C          — cancel generation / quit"
                        .to_string(),
                ));
            }
            "/add" => {
                if let Some(path) = parts.get(1).map(|s| s.trim()) {
                    match std::fs::read_to_string(path) {
                        Ok(contents) => {
                            self.session.push(Message::system(format!(
                                "File `{path}`:\n```\n{contents}\n```"
                            )));
                            self.session
                                .push(Message::assistant(format!("Added `{path}` to context.")));
                        }
                        Err(e) => {
                            self.session
                                .push(Message::assistant(format!("[error reading {path}] {e}")));
                        }
                    }
                } else {
                    self.session
                        .push(Message::assistant("Usage: /add <file-path>".to_string()));
                }
            }
            _ => {
                self.session.push(Message::assistant(format!(
                    "Unknown command: {cmd}\nType /help for available commands."
                )));
            }
        }
        Ok(())
    }
}

pub async fn run_tui(backend: Arc<dyn Backend>, args: &Args) -> color_eyre::Result<()> {
    let mut terminal = ratatui::init();
    let (tx, rx) = mpsc::channel(64);
    let result = App::new(backend, args).run(&mut terminal, tx, rx).await;
    ratatui::restore();
    result
}
