use ratatui::layout::{Alignment, Position, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::{SpineHintTone, SpineMode, SpineState};
use crate::ui::theme;

const PLACEHOLDER_INTENT: &str = "Type intent...";

pub fn render(frame: &mut Frame<'_>, area: Rect, state: &SpineState) {
    let block = spine_block();
    let inner = block.inner(area);

    let hint_text = state.hint.text.as_str();
    let hint_tone = state.hint.tone;

    let mut spans = build_spans(state);
    let mut lines = Vec::new();
    let mut hint_on_first_line = false;
    if hint_fits(&spans, inner.width, hint_text) {
        append_hint(&mut spans, inner.width, hint_text, hint_tone);
        hint_on_first_line = true;
    }
    lines.push(Line::from(spans));

    if !hint_on_first_line && !hint_text.is_empty() && inner.height >= 2 {
        lines.push(build_hint_line(inner.width, hint_text, hint_tone));
    }

    let widget = Paragraph::new(lines)
        .block(block)
        .alignment(Alignment::Left);
    frame.render_widget(widget, area);
}

pub fn set_cursor(frame: &mut Frame<'_>, area: Rect, state: &SpineState) {
    let block = spine_block();
    let inner = block.inner(area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let label_len = mode_label(state.mode).len() as u16;
    let prefix_len = command_prefix(state.mode).len() as u16;
    let base_x = inner.x + 1 + label_len + 1 + prefix_len;
    let cursor_x = base_x.saturating_add(state.input.cursor as u16);
    let max_x = inner.x + inner.width.saturating_sub(1);
    let cursor_x = cursor_x.min(max_x);

    frame.set_cursor_position(Position::new(cursor_x, inner.y));
}

fn spine_block<'a>() -> Block<'a> {
    Block::default()
        .borders(Borders::TOP)
        .border_style(theme::spine_border_style())
}

fn mode_label(mode: SpineMode) -> &'static str {
    match mode {
        SpineMode::Intent => "Intent",
        SpineMode::Command => "Command",
        SpineMode::WhisperCluster => "Whisper Cluster",
        SpineMode::WhisperAgent => "Whisper Agent",
    }
}

fn command_prefix(mode: SpineMode) -> &'static str {
    match mode {
        SpineMode::Command => "/",
        _ => "",
    }
}

fn build_spans<'a>(state: &'a SpineState) -> Vec<Span<'a>> {
    let mut spans = Vec::new();
    push_mode_label(&mut spans, state.mode);
    push_prefix(&mut spans, state.mode);
    push_input_or_placeholder(&mut spans, state);
    push_completion(&mut spans, state);
    spans
}

fn push_mode_label<'a>(spans: &mut Vec<Span<'a>>, mode: SpineMode) {
    spans.push(Span::raw(" "));
    spans.push(Span::styled(mode_label(mode), theme::spine_mode_style()));
    spans.push(Span::raw(" "));
}

fn push_prefix<'a>(spans: &mut Vec<Span<'a>>, mode: SpineMode) {
    let prefix = command_prefix(mode);
    if !prefix.is_empty() {
        spans.push(Span::styled(prefix, theme::spine_prefix_style()));
    }
}

fn push_input_or_placeholder<'a>(spans: &mut Vec<Span<'a>>, state: &'a SpineState) {
    if state.input.input.is_empty() {
        if matches!(state.mode, SpineMode::Intent) {
            spans.push(Span::styled(
                PLACEHOLDER_INTENT,
                theme::spine_placeholder_style(),
            ));
        }
        return;
    }

    spans.push(Span::styled(
        state.input.input.as_str(),
        theme::spine_input_style(),
    ));
}

fn push_completion<'a>(spans: &mut Vec<Span<'a>>, state: &'a SpineState) {
    let Some(completion) = &state.completion else {
        return;
    };
    if completion.ghost.is_empty() || state.input.input.is_empty() {
        return;
    }
    spans.push(Span::styled(
        completion.ghost.as_str(),
        theme::spine_completion_style(),
    ));
}

fn hint_fits(spans: &[Span<'_>], width: u16, hint: &str) -> bool {
    if hint.is_empty() || width == 0 {
        return false;
    }
    let used_len: usize = spans.iter().map(|span| span.content.len()).sum();
    let hint_len = hint.len();
    let width = width as usize;
    width > used_len + hint_len + 1
}

fn append_hint<'a>(spans: &mut Vec<Span<'a>>, width: u16, hint: &'a str, tone: SpineHintTone) {
    if hint.is_empty() || width == 0 {
        return;
    }
    let used_len: usize = spans.iter().map(|span| span.content.len()).sum();
    let hint_len = hint.len();
    let width = width as usize;
    if width <= used_len + hint_len + 1 {
        return;
    }
    let gap = width - used_len - hint_len;
    spans.push(Span::raw(" ".repeat(gap)));
    spans.push(Span::styled(hint, theme::spine_hint_style_for(tone)));
}

fn build_hint_line<'a>(width: u16, hint: &'a str, tone: SpineHintTone) -> Line<'a> {
    if width == 0 {
        return Line::from(Span::raw(""));
    }
    let width = width as usize;
    let hint_len = hint.len();
    if hint_len >= width {
        return Line::from(Span::styled(hint, theme::spine_hint_style_for(tone)));
    }
    let gap = width - hint_len;
    Line::from(vec![
        Span::raw(" ".repeat(gap)),
        Span::styled(hint, theme::spine_hint_style_for(tone)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Position;
    use ratatui::Terminal;

    use crate::ui::widgets::test_utils::line_text;

    #[test]
    fn intent_mode_shows_placeholder() {
        let backend = TestBackend::new(40, 3);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let state = SpineState::default();

        terminal
            .draw(|frame| {
                let area = frame.area();
                render(frame, area, &state);
            })
            .expect("draw");

        let buffer = terminal.backend().buffer();
        let content = line_text(buffer, 1);
        assert!(content.contains(PLACEHOLDER_INTENT));
    }

    #[test]
    fn command_mode_prefix_is_rendered() {
        let backend = TestBackend::new(40, 3);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut state = SpineState::default();
        state.mode = SpineMode::Command;
        state.input.input = "help".to_string();

        terminal
            .draw(|frame| {
                let area = frame.area();
                render(frame, area, &state);
            })
            .expect("draw");

        let buffer = terminal.backend().buffer();
        let content = line_text(buffer, 1);
        assert!(content.contains("/help"));
    }

    #[test]
    fn spine_cursor_follows_input() {
        let backend = TestBackend::new(40, 3);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut state = SpineState::default();
        state.input.input = "abcd".to_string();
        state.input.cursor = 2;

        terminal
            .draw(|frame| {
                let area = frame.area();
                render(frame, area, &state);
                set_cursor(frame, area, &state);
            })
            .expect("draw");

        let label_len = mode_label(state.mode).len() as u16;
        let base_x = 1 + label_len + 1;
        let expected_x = base_x + state.input.cursor as u16;
        terminal
            .backend_mut()
            .assert_cursor_position(Position::new(expected_x, 1));
    }

    #[test]
    fn hint_moves_to_second_line_when_no_space() {
        let backend = TestBackend::new(22, 4);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut state = SpineState::default();
        state.input.input = "extremelylonginput".to_string();
        state.hint = crate::app::SpineHint::new("Second line hint", SpineHintTone::Info);

        terminal
            .draw(|frame| {
                let area = frame.area();
                render(frame, area, &state);
            })
            .expect("draw");

        let buffer = terminal.backend().buffer();
        let line1 = line_text(buffer, 1);
        let line2 = line_text(buffer, 2);
        assert!(!line1.contains("Second line hint"));
        assert!(line2.contains("Second line hint"));
    }

    #[test]
    fn completion_renders_dimmed_after_input() {
        let backend = TestBackend::new(40, 3);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut state = SpineState::default();
        state.mode = SpineMode::Command;
        state.input.input = "pro".to_string();
        state.input.cursor = 3;
        state.completion = Some(crate::app::SpineCompletion {
            candidates: vec!["provider".to_string()],
            selected: 0,
            ghost: "vider".to_string(),
        });

        terminal
            .draw(|frame| {
                let area = frame.area();
                render(frame, area, &state);
            })
            .expect("draw");

        let buffer = terminal.backend().buffer();
        let line = line_text(buffer, 1);
        let ghost_start = line.find("vider").expect("ghost text");
        let cell = buffer.cell((ghost_start as u16, 1)).expect("ghost cell");
        let expected_fg = theme::spine_completion_style().fg.expect("fg");
        assert_eq!(cell.fg, expected_fg);
    }
}
