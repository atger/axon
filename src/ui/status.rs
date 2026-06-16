use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

pub enum GenState {
    Idle,
    Generating,
    Connecting,
}

pub fn render(frame: &mut Frame, area: Rect, model: &str, branch: Option<&str>, state: &GenState) {
    let status = match state {
        GenState::Idle => Span::styled("ready", Style::default().fg(Color::Green)),
        GenState::Generating => Span::styled(
            "generating…",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        GenState::Connecting => Span::styled(
            "connecting…",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
    };

    let branch_span = match branch {
        Some(b) => Span::styled(format!(" {b} "), Style::default().fg(Color::Cyan)),
        None => Span::styled(" – ", Style::default().fg(Color::DarkGray)),
    };

    let line = Line::from(vec![
        Span::styled(" axon ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("│"),
        Span::styled(format!(" {model} "), Style::default().fg(Color::Magenta)),
        Span::raw("│"),
        branch_span,
        Span::raw("│ "),
        status,
        Span::raw(" "),
    ]);

    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(Color::DarkGray)),
        area,
    );
}
