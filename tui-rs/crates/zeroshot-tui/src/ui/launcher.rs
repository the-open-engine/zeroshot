use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::screens::launcher::State;

pub fn render(frame: &mut Frame<'_>, area: Rect, state: &State, provider_override: Option<&str>) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(3)])
        .split(area);

    let provider_label = provider_override.unwrap_or("default");
    let hint_lines = vec![
        Line::from("Type a task and press Enter."),
        Line::from(format!(
            "Commands: /monitor  /help  provider: {provider_label}"
        )),
    ];
    let hints = Paragraph::new(hint_lines).block(Block::default().borders(Borders::ALL));
    frame.render_widget(hints, chunks[0]);

    let input = Paragraph::new(state.input.as_str())
        .block(Block::default().borders(Borders::ALL).title("Input"));
    frame.render_widget(input, chunks[1]);

    if chunks[1].height > 2 && chunks[1].width > 2 {
        let max_x = chunks[1].x + chunks[1].width.saturating_sub(2);
        let cursor_x = chunks[1].x + 1 + state.cursor as u16;
        let cursor_x = cursor_x.min(max_x);
        let cursor_y = chunks[1].y + 1;
        frame.set_cursor(cursor_x, cursor_y);
    }
}
