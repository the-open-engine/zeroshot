use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::Terminal;

use zeroshot_tui::app::{AppState, UiVariant};
use zeroshot_tui::ui;

fn buffer_text(buffer: &Buffer) -> String {
    let area = buffer.area;
    let mut lines = Vec::new();
    for y in area.top()..area.bottom() {
        let mut line = String::new();
        for x in area.left()..area.right() {
            line.push_str(buffer.cell((x, y)).map_or("", |c| c.symbol()));
        }
        lines.push(line);
    }
    lines.join("\n")
}

#[test]
fn disruptive_render_draws_canvas_and_spine_placeholders() {
    let backend = TestBackend::new(60, 12);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let mut state = AppState::default();
    state.ui_variant = UiVariant::Disruptive;

    terminal
        .draw(|frame| ui::render(frame, &state))
        .expect("draw");

    let content = buffer_text(terminal.backend().buffer());
    assert!(content.contains("Canvas"));
    assert!(content.contains("Spine"));
    assert!(!content.contains("ZEROSHOT"));
}
