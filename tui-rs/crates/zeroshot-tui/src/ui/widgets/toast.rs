use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::{ToastLevel, ToastState};

pub fn render(frame: &mut Frame<'_>, area: Rect, toast: Option<&ToastState>) {
    let (label, style, message) = match toast {
        Some(toast) => match toast.level {
            ToastLevel::Info => ("Info", Style::default().fg(Color::Blue), toast.message.as_str()),
            ToastLevel::Success => (
                "Success",
                Style::default().fg(Color::Green),
                toast.message.as_str(),
            ),
            ToastLevel::Error => ("Error", Style::default().fg(Color::Red), toast.message.as_str()),
        },
        None => ("Ready", Style::default().fg(Color::Green), "Idle"),
    };

    let mut message_lines = message.lines();
    let first_line = message_lines.next().unwrap_or("");
    let mut lines = vec![Line::from(vec![
        Span::styled("Status:", Style::default().fg(Color::DarkGray)),
        Span::raw(" "),
        Span::styled(label, style.add_modifier(Modifier::BOLD)),
        Span::raw("  "),
        Span::raw(first_line),
    ])];

    for line in message_lines {
        lines.push(Line::from(line));
    }

    let widget = Paragraph::new(lines).block(Block::default().borders(Borders::TOP));
    frame.render_widget(widget, area);
}
