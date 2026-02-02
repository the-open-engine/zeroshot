use ratatui::layout::{Alignment, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::ui::theme;

pub fn render(frame: &mut Frame<'_>, area: Rect, cluster_id: &str, agent_id: &str) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Agent Microscope");

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(agent_id, theme::title_style())),
        Line::from(Span::styled(
            format!("Cluster {cluster_id}"),
            theme::muted_style(),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Microscope not implemented yet",
            theme::dim_style(),
        )),
        Line::from(""),
        Line::from(Span::styled("Press Esc to return", theme::dim_style())),
    ];

    let widget = Paragraph::new(lines)
        .alignment(Alignment::Center)
        .block(block);
    frame.render_widget(widget, area);
}
