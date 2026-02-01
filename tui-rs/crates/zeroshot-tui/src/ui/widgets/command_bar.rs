use ratatui::layout::Rect;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::CommandBarState;

pub fn render(frame: &mut Frame<'_>, area: Rect, state: &CommandBarState) {
    let title = if state.active { "Command" } else { "Command (/)" };
    let content = if state.active { state.input.as_str() } else { "" };
    let widget = Paragraph::new(content).block(Block::default().borders(Borders::ALL).title(title));
    frame.render_widget(widget, area);
}

pub fn set_cursor(frame: &mut Frame<'_>, area: Rect, state: &CommandBarState) {
    if !state.active {
        return;
    }
    if area.height <= 2 || area.width <= 2 {
        return;
    }

    let max_x = area.x + area.width.saturating_sub(2);
    let cursor_x = area.x + 1 + state.cursor as u16;
    let cursor_x = cursor_x.min(max_x);
    let cursor_y = area.y + 1;
    frame.set_cursor(cursor_x, cursor_y);
}
