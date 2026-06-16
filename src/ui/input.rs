use ratatui::{Frame, layout::Rect};
use tui_textarea::TextArea;

pub struct InputWidget<'a> {
    pub textarea: TextArea<'a>,
    history: Vec<String>,
    history_pos: Option<usize>,
    /// Text saved when navigating history so we can restore it
    saved_input: String,
}

impl<'a> InputWidget<'a> {
    pub fn new() -> Self {
        let mut textarea = TextArea::default();
        textarea.set_placeholder_text("Ask anything… (/help for commands, @file to attach)");
        Self {
            textarea,
            history: Vec::new(),
            history_pos: None,
            saved_input: String::new(),
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
        self.history_pos = None;
        self.saved_input.clear();
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
        self.history_pos = None;
        self.saved_input.clear();
    }

    pub fn history_up(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let new_pos = match self.history_pos {
            None => {
                self.saved_input = self.text();
                self.history.len() - 1
            }
            Some(p) if p > 0 => p - 1,
            Some(p) => p,
        };
        self.history_pos = Some(new_pos);
        self.set_text(self.history[new_pos].clone());
    }

    pub fn history_down(&mut self) {
        match self.history_pos {
            None => {}
            Some(p) if p + 1 < self.history.len() => {
                let next = p + 1;
                self.history_pos = Some(next);
                self.set_text(self.history[next].clone());
            }
            Some(_) => {
                self.history_pos = None;
                let saved = self.saved_input.clone();
                self.set_text(saved);
            }
        }
    }

    fn set_text(&mut self, text: String) {
        self.textarea = TextArea::from(text.lines().map(|l| l.to_string()));
        self.textarea
            .set_placeholder_text("Ask anything… (/help for commands, @file to attach)");
        // Move cursor to end
        self.textarea.move_cursor(tui_textarea::CursorMove::End);
    }

    /// Forward a crossterm key event to the textarea.
    pub fn input(&mut self, event: crossterm::event::KeyEvent) -> bool {
        self.textarea.input(event)
    }
}
