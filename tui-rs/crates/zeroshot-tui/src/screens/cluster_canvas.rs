use ratatui::layout::{Alignment, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::ui::theme;

pub fn render(frame: &mut Frame<'_>, area: Rect, cluster_id: &str) {
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("Cluster {cluster_id}"),
            theme::title_style(),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Cluster canvas not implemented yet",
            theme::muted_style(),
        )),
        Line::from(Span::styled(
            "Press Esc to return to Fleet Radar",
            theme::dim_style(),
        )),
    ];
    let widget = Paragraph::new(lines)
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Cluster Canvas"),
        );
    frame.render_widget(widget, area);
}
