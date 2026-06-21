use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
    MouseEventKind,
};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::Line,
    widgets::{Block, Borders, Clear, Paragraph},
};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::agent::{AgentLoop, ConfirmFn};
use crate::cli::BackendKind;
use crate::context::ContextProvider;
use crate::llm::{Backend, StreamEvent, daemon::DaemonBackend, ollama::OllamaBackend};
use crate::session::{ConversationHistory, Message};
use crate::tools::ToolRegistry;
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
    ToolConfirmRequest {
        tool_name: String,
        args_summary: String,
        confirm_tx: oneshot::Sender<bool>,
    },
}

enum Generating {
    Idle,
    Active {
        cancel: CancellationToken,
        partial: String,
    },
    AwaitingConfirm {
        cancel: CancellationToken,
        tool_name: String,
        args_summary: String,
        confirm_tx: oneshot::Sender<bool>,
    },
}

pub struct App<'a> {
    running: bool,
    session: ConversationHistory,
    backend: Arc<dyn Backend>,
    // Stored so /model can rebuild the backend without re-parsing CLI args.
    backend_kind: BackendKind,
    ollama_url: String,
    num_ctx: Option<usize>,
    no_download: bool,
    context: ContextProvider,
    tools: Arc<ToolRegistry>,
    chat: ChatWidget,
    input: InputWidget<'a>,
    generating: Generating,
    /// True while waiting for the daemon to start (e.g. during /model switch).
    connecting: bool,
    /// User scrolled up manually; suppress auto-scroll-to-bottom while true.
    user_scrolled: bool,
}

impl<'a> App<'a> {
    fn new(
        backend: Arc<dyn Backend>,
        backend_kind: BackendKind,
        ollama_url: String,
        no_download: bool,
        context_window: Option<usize>,
        skill_content: Option<String>,
    ) -> Self {
        let context = ContextProvider::new(skill_content);
        let cw = context_window.unwrap_or_else(|| backend.context_window());
        Self {
            running: true,
            session: ConversationHistory::new(cw),
            backend,
            backend_kind,
            ollama_url,
            num_ctx: context_window,
            no_download,
            context,
            tools: Arc::new(ToolRegistry::with_defaults()),
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
                Generating::Active { .. } | Generating::AwaitingConfirm { .. } => {
                    GenState::Generating
                }
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
            _ => None,
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

        // Confirmation overlay — rendered on top when awaiting user approval.
        if let Generating::AwaitingConfirm {
            tool_name,
            args_summary,
            ..
        } = &self.generating
        {
            render_confirm_overlay(frame, area, tool_name, args_summary);
        }
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
            AppEvent::Crossterm(Event::Mouse(mouse)) => match mouse.kind {
                MouseEventKind::ScrollUp => {
                    self.user_scrolled = true;
                    self.chat.scroll_up(3);
                }
                MouseEventKind::ScrollDown => {
                    self.chat.scroll_down(3);
                    if self.chat.scroll_offset == 0 {
                        self.user_scrolled = false;
                    }
                }
                _ => {}
            },
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
                if let Generating::Active { .. } | Generating::AwaitingConfirm { .. } =
                    std::mem::replace(&mut self.generating, Generating::Idle)
                {
                    self.session
                        .push(Message::assistant(format!("[error] {e}")));
                }
            }

            AppEvent::ToolConfirmRequest {
                tool_name,
                args_summary,
                confirm_tx,
            } => {
                if let Generating::Active { cancel, .. } =
                    std::mem::replace(&mut self.generating, Generating::Idle)
                {
                    self.generating = Generating::AwaitingConfirm {
                        cancel,
                        tool_name,
                        args_summary,
                        confirm_tx,
                    };
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
        // While awaiting tool confirmation, only y/n/Esc/Ctrl+C are handled.
        if matches!(self.generating, Generating::AwaitingConfirm { .. }) {
            match (key.modifiers, key.code) {
                (KeyModifiers::CONTROL, KeyCode::Char('c') | KeyCode::Char('C')) => {
                    self.cancel_with_deny();
                }
                (_, KeyCode::Char('y') | KeyCode::Char('Y')) => {
                    self.resolve_confirm(true);
                }
                (_, KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc) => {
                    self.resolve_confirm(false);
                }
                _ => {}
            }
            return Ok(());
        }

        // Global shortcuts take precedence
        match (key.modifiers, key.code) {
            (KeyModifiers::CONTROL, KeyCode::Char('c') | KeyCode::Char('C')) => {
                match std::mem::replace(&mut self.generating, Generating::Idle) {
                    Generating::Active { cancel, .. } => {
                        cancel.cancel();
                        // StreamDone will fire and finalize
                    }
                    Generating::AwaitingConfirm {
                        cancel, confirm_tx, ..
                    } => {
                        cancel.cancel();
                        let _ = confirm_tx.send(false);
                    }
                    Generating::Idle => self.running = false,
                }
                return Ok(());
            }
            (KeyModifiers::CONTROL, KeyCode::Char('d') | KeyCode::Char('D')) => {
                self.running = false;
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
            (_, KeyCode::Up) => {
                // Scroll chat when input is empty or single-line; otherwise move textarea cursor
                if self.input.text().trim().is_empty() || !self.input.text().contains('\n') {
                    self.user_scrolled = true;
                    self.chat.scroll_up(1);
                    return Ok(());
                }
            }
            (_, KeyCode::Down) => {
                if self.input.text().trim().is_empty() || !self.input.text().contains('\n') {
                    self.chat.scroll_down(1);
                    if self.chat.scroll_offset == 0 {
                        self.user_scrolled = false;
                    }
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

    /// Sends `confirmed` to the agent and restores the Active generating state.
    fn resolve_confirm(&mut self, confirmed: bool) {
        if let Generating::AwaitingConfirm {
            cancel, confirm_tx, ..
        } = std::mem::replace(&mut self.generating, Generating::Idle)
        {
            let _ = confirm_tx.send(confirmed);
            self.generating = Generating::Active {
                cancel,
                partial: String::new(),
            };
        }
    }

    /// Cancels the agent and denies any pending confirmation.
    fn cancel_with_deny(&mut self) {
        if let Generating::AwaitingConfirm {
            cancel, confirm_tx, ..
        } = std::mem::replace(&mut self.generating, Generating::Idle)
        {
            cancel.cancel();
            let _ = confirm_tx.send(false);
        }
    }

    async fn handle_submit(&mut self, app_tx: &mpsc::Sender<AppEvent>) -> color_eyre::Result<()> {
        if matches!(
            self.generating,
            Generating::Active { .. } | Generating::AwaitingConfirm { .. }
        ) {
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

        // Expand @path references anywhere in the message.
        let user_text = expand_at_files(&text);

        self.session.push(Message::user(user_text));
        self.chat.scroll_to_bottom();

        // Assemble system prompt including tool instructions.
        let system_prompt = format!(
            "{}\n{}",
            self.context.system_prompt(),
            self.tools.system_prompt_section()
        );
        let messages = self.session.assemble(&system_prompt);
        let (stream_tx, mut stream_rx) = mpsc::channel::<StreamEvent>(64);
        let cancel = CancellationToken::new();

        self.generating = Generating::Active {
            cancel: cancel.clone(),
            partial: String::new(),
        };

        let backend = Arc::clone(&self.backend);
        let tools = Arc::clone(&self.tools);
        let app_tx_confirm = app_tx.clone();
        let app_tx2 = app_tx.clone();
        let cancel2 = cancel.clone();

        tokio::spawn(async move {
            let confirm: ConfirmFn = Box::new(move |tool_name, args_summary| {
                let tx = app_tx_confirm.clone();
                Box::pin(async move {
                    let (conf_tx, conf_rx) = oneshot::channel::<bool>();
                    let _ = tx
                        .send(AppEvent::ToolConfirmRequest {
                            tool_name,
                            args_summary,
                            confirm_tx: conf_tx,
                        })
                        .await;
                    conf_rx.await.unwrap_or(false)
                })
            });

            let agent = AgentLoop::new(backend, tools);
            if let Err(e) = agent.run(messages, cancel2, &confirm, stream_tx).await {
                let _ = app_tx2.send(AppEvent::StreamError(e.to_string())).await;
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
                        BackendKind::Ollama => {
                            Arc::new(OllamaBackend::new(&self.ollama_url, name, self.num_ctx))
                        }
                        BackendKind::Local => {
                            self.connecting = true;
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
                    let mut cfg = crate::config::AxonConfig::load();
                    cfg.model = Some(name.to_string());
                    let _ = cfg.save();
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

/// Expands `@path` tokens anywhere in `text` by replacing them with the file's
/// contents inline. Unreadable paths are left as-is so the model still sees them.
fn expand_at_files(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.char_indices().peekable();
    while let Some((_, c)) = chars.next() {
        if c == '@' {
            let path: String = chars
                .by_ref()
                .take_while(|(_, c)| !c.is_whitespace())
                .map(|(_, c)| c)
                .collect();
            if path.is_empty() {
                result.push('@');
            } else {
                match std::fs::read_to_string(&path) {
                    Ok(contents) => {
                        result.push_str(&format!("[File `{path}`:\n```\n{contents}\n```]"));
                    }
                    Err(_) => {
                        result.push('@');
                        result.push_str(&path);
                    }
                }
            }
        } else {
            result.push(c);
        }
    }
    result
}

/// Renders a centered confirmation dialog over the existing UI.
fn render_confirm_overlay(frame: &mut Frame, area: Rect, tool_name: &str, args_summary: &str) {
    let width = area.width.min(60);
    let height = 7u16;
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    let popup = Rect::new(x, y, width, height);

    // Truncate args_summary to fit within the dialog width (accounting for borders + padding).
    let inner_width = width.saturating_sub(4) as usize;
    let args_display = if args_summary.len() > inner_width {
        format!("{}…", &args_summary[..inner_width.saturating_sub(1)])
    } else {
        args_summary.to_string()
    };

    let lines = vec![
        Line::from(""),
        Line::styled(
            format!("  Run: {tool_name}"),
            Style::default().fg(Color::Yellow),
        ),
        Line::from(format!("  {args_display}")),
        Line::from(""),
        Line::styled(
            "  [y] confirm   [n / Esc] cancel",
            Style::default().fg(Color::Cyan),
        ),
    ];

    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Confirm tool call ")
                    .title_alignment(Alignment::Center)
                    .border_style(Style::default().fg(Color::Yellow)),
            )
            .alignment(Alignment::Left),
        popup,
    );
}

pub async fn run_tui(
    backend: Arc<dyn Backend>,
    backend_kind: BackendKind,
    ollama_url: String,
    no_download: bool,
    context_window: Option<usize>,
    skill_content: Option<String>,
) -> color_eyre::Result<()> {
    let mut terminal = ratatui::init();
    crossterm::execute!(std::io::stdout(), EnableMouseCapture)?;
    let (tx, rx) = mpsc::channel(64);
    let result = App::new(
        backend,
        backend_kind,
        ollama_url,
        no_download,
        context_window,
        skill_content,
    )
    .run(&mut terminal, tx, rx)
    .await;
    crossterm::execute!(std::io::stdout(), DisableMouseCapture).ok();
    ratatui::restore();
    result
}
