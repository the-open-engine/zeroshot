use ratatui::layout::{Position, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::CommandBarState;
use crate::ui::theme;

/// Render the command bar as a single line (replaces status bar when active).
pub fn render(frame: &mut Frame<'_>, area: Rect, state: &CommandBarState, _allow_open: bool) {
    if !state.active {
        return;
    }

    let line = Line::from(vec![
        Span::raw(" "),
        Span::styled("/", theme::key_style()),
        Span::styled(state.input(), theme::title_style()),
    ]);

    let widget = Paragraph::new(line);
    frame.render_widget(widget, area);
}

/// Set cursor position for the command bar input.
pub fn set_cursor(frame: &mut Frame<'_>, area: Rect, state: &CommandBarState) {
    if !state.active {
        return;
    }
    if area.width <= 3 {
        return;
    }

    // Offset: 1 (padding) + 1 (/) + cursor position
    let max_x = area.x + area.width.saturating_sub(1);
    let cursor_x = area.x + 2 + state.cursor() as u16;
    let cursor_x = cursor_x.min(max_x);
    frame.set_cursor_position(Position::new(cursor_x, area.y));
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    use crate::ui::widgets::test_utils::line_text;

    #[test]
    fn active_command_bar_renders_input() {
        let backend = TestBackend::new(40, 1);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut state = CommandBarState::default();
        state.open_with("help".to_string());

        terminal
            .draw(|frame| {
                let area = frame.area();
                render(frame, area, &state, true);
            })
            .expect("draw");

        let buffer = terminal.backend().buffer();
        let content = line_text(buffer, 0);
        assert!(content.contains("/help"));
    }
}
