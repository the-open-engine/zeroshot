use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::{ToastLevel, ToastState};

pub fn render(frame: &mut Frame<'_>, area: Rect, toast: Option<&ToastState>) {
    let (message, title, style) = match toast {
        Some(toast) => {
            let (label, style) = match toast.level {
                ToastLevel::Info => ("Info", Style::default().fg(Color::Blue)),
                ToastLevel::Success => ("Success", Style::default().fg(Color::Green)),
                ToastLevel::Error => ("Error", Style::default().fg(Color::Red)),
            };
            (toast.message.as_str(), format!("Status ({label})"), style)
        }
        None => ("", "Status".to_string(), Style::default()),
    };

    let widget = Paragraph::new(message)
        .style(style)
        .block(Block::default().borders(Borders::ALL).title(title));
    frame.render_widget(widget, area);
}
