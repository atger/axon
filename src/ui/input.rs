use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::Paragraph,
};
use tui_textarea::TextArea;

pub struct InputWidget<'a> {
    pub textarea: TextArea<'a>,
    history: Vec<String>,
}

fn init_textarea() -> TextArea<'static> {
    let mut ta = TextArea::default();
    ta.set_placeholder_text("Ask anything… (/help for commands, @file to attach)");
    ta.set_cursor_style(Style::default().add_modifier(Modifier::REVERSED));
    ta.set_placeholder_style(Style::default().fg(Color::DarkGray));
    ta
}

impl<'a> InputWidget<'a> {
    pub fn new() -> Self {
        Self {
            textarea: init_textarea(),
            history: Vec::new(),
        }
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        use ratatui::widgets::{Block, Borders};

        // Outer block owns the top/bottom borders; textarea gets none of its own.
        let outer = Block::default().borders(Borders::TOP | Borders::BOTTOM);
        let inner = outer.inner(area);
        frame.render_widget(outer, area);

        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(2), Constraint::Min(1)])
            .split(inner);

        frame.render_widget(Paragraph::new("❯ "), cols[0]);
        self.textarea
            .set_block(Block::default().borders(Borders::NONE));
        frame.render_widget(&self.textarea, cols[1]);
    }

    /// Number of content lines currently in the textarea (minimum 1).
    pub fn line_count(&self) -> usize {
        self.textarea.lines().len().max(1)
    }

    /// Returns the current input text.
    pub fn text(&self) -> String {
        self.textarea.lines().join("\n")
    }

    /// Clears the input and pushes the text into history.
    pub fn submit(&mut self) -> String {
        let text = self.text();
        if !text.trim().is_empty() {
            self.history.push(text.clone());
        }
        self.textarea = init_textarea();
        text
    }

    /// Clears without saving to history.
    pub fn clear(&mut self) {
        self.textarea = init_textarea();
    }

    /// Shell-style line continuation: removes trailing `\` and inserts a newline.
    pub fn do_continuation(&mut self) {
        self.textarea
            .input(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        self.textarea
            .input(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
    }

    /// Forward a crossterm key event to the textarea.
    pub fn input(&mut self, event: KeyEvent) -> bool {
        self.textarea.input(event)
    }
}
