use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::CommandBarState;

pub fn render(frame: &mut Frame<'_>, area: Rect, state: &CommandBarState, allow_open: bool) {
    let (title, content, style) = if state.active {
        (
            "Command",
            state.input.as_str(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
    } else if allow_open {
        (
            "Command (/)",
            "Press / for commands",
            Style::default().fg(Color::DarkGray),
        )
    } else {
        (
            "Command",
            "Launcher: type task above",
            Style::default().fg(Color::DarkGray),
        )
    };

    let widget = Paragraph::new(content).block(
        Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(style),
    );
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::Terminal;

    fn line_text(buffer: &Buffer, y: u16) -> String {
        let area = buffer.area;
        let mut line = String::new();
        for x in area.left()..area.right() {
            line.push_str(buffer.get(x, y).symbol());
        }
        line
    }

    #[test]
    fn inactive_command_bar_shows_hint() {
        let backend = TestBackend::new(40, 3);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let state = CommandBarState::default();

        terminal
            .draw(|frame| {
                let area = frame.size();
                render(frame, area, &state, true);
            })
            .expect("draw");

        let buffer = terminal.backend().buffer();
        let content = line_text(buffer, 1);
        assert!(content.contains("Press / for commands"));
    }

    #[test]
    fn active_command_bar_renders_input() {
        let backend = TestBackend::new(40, 3);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut state = CommandBarState::default();
        state.active = true;
        state.input = "/help".to_string();
        state.cursor = 5;

        terminal
            .draw(|frame| {
                let area = frame.size();
                render(frame, area, &state, true);
            })
            .expect("draw");

        let buffer = terminal.backend().buffer();
        let title = line_text(buffer, 0);
        let content = line_text(buffer, 1);
        assert!(title.contains("Command"));
        assert!(content.contains("/help"));
    }
}
