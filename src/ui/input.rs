use ratatui::{Frame, layout::Rect};
use tui_textarea::TextArea;

pub struct InputWidget<'a> {
    pub textarea: TextArea<'a>,
    history: Vec<String>,
}

impl<'a> InputWidget<'a> {
    pub fn new() -> Self {
        let mut textarea = TextArea::default();
        textarea.set_placeholder_text("Ask anything… (/help for commands, @file to attach)");
        Self {
            textarea,
            history: Vec::new(),
        }
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        use ratatui::widgets::{Block, Borders};
        self.textarea
            .set_block(Block::default().borders(Borders::ALL).title(" input "));
        frame.render_widget(&self.textarea, area);
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
        self.textarea = TextArea::default();
        self.textarea
            .set_placeholder_text("Ask anything… (/help for commands, @file to attach)");
        text
    }

    /// Clears without saving to history.
    pub fn clear(&mut self) {
        self.textarea = TextArea::default();
        self.textarea
            .set_placeholder_text("Ask anything… (/help for commands, @file to attach)");
    }

    /// Forward a crossterm key event to the textarea.
    pub fn input(&mut self, event: crossterm::event::KeyEvent) -> bool {
        self.textarea.input(event)
    }
}
