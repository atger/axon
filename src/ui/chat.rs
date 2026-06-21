use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};

use crate::session::{Message, Role};

pub struct ChatWidget {
    pub scroll_offset: u16,
}

impl ChatWidget {
    pub fn new() -> Self {
        Self { scroll_offset: 0 }
    }

    pub fn scroll_up(&mut self, n: u16) {
        self.scroll_offset = self.scroll_offset.saturating_add(n);
    }

    pub fn scroll_down(&mut self, n: u16) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
    }

    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
    }

    pub fn render(
        &self,
        frame: &mut Frame,
        area: Rect,
        messages: &[Message],
        streaming: Option<&str>,
    ) {
        let mut lines: Vec<Line> = Vec::new();

        for msg in messages {
            match msg.role {
                Role::System => {
                    lines.push(Line::from(Span::styled(
                        format!("[system] {}", msg.content.trim()),
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::ITALIC),
                    )));
                    lines.push(Line::default());
                }
                Role::User => {
                    let w = area.width as usize;
                    for text_line in msg.content.lines() {
                        let content = format!("  {text_line}");
                        lines.push(Line::styled(
                            format!("{content:<w$}"),
                            Style::default().bg(Color::DarkGray).fg(Color::White),
                        ));
                    }
                    lines.push(Line::default());
                }
                Role::Assistant => {
                    render_content(&msg.content, &mut lines);
                    lines.push(Line::default());
                }
            }
        }

        // In-progress streaming message
        if let Some(partial) = streaming {
            render_content(partial, &mut lines);
            // Blinking cursor
            lines.push(Line::from(Span::styled(
                "  ▋",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::RAPID_BLINK),
            )));
        }

        let total_lines = lines.len() as u16;
        let visible = area.height;
        let max_scroll = total_lines.saturating_sub(visible);
        let scroll = self.scroll_offset.min(max_scroll);
        // Ratatui scroll is (row_from_top, col), we scroll from bottom
        let scroll_from_top = max_scroll.saturating_sub(scroll);

        frame.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .scroll((scroll_from_top, 0)),
            area,
        );
    }
}

/// Render message content, detecting fenced code blocks.
fn render_content(content: &str, lines: &mut Vec<Line>) {
    let mut in_code = false;
    let mut lang = String::new();

    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("```") {
            if in_code {
                // closing fence
                lines.push(Line::from(Span::styled(
                    "  ╰────────────────────────────────────────╯",
                    Style::default().fg(Color::DarkGray),
                )));
                in_code = false;
                lang.clear();
            } else {
                // opening fence
                lang = rest.trim().to_string();
                let header = if lang.is_empty() {
                    "  ╭─────────────────────────────────────────╮".to_string()
                } else {
                    format!("  ╭─ {lang} ─")
                };
                lines.push(Line::from(Span::styled(
                    header,
                    Style::default().fg(Color::DarkGray),
                )));
                in_code = true;
            }
        } else if in_code {
            lines.push(Line::from(vec![
                Span::styled("  │ ", Style::default().fg(Color::DarkGray)),
                Span::styled(line.to_string(), Style::default().fg(Color::White)),
            ]));
        } else {
            lines.push(Line::from(format!("  {line}")));
        }
    }

    // Unclosed code block (streaming)
    if in_code {
        lines.push(Line::from(Span::styled(
            "  │",
            Style::default().fg(Color::DarkGray),
        )));
    }
}
