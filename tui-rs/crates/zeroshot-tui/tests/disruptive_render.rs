use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::Terminal;

use zeroshot_tui::app::{AppState, UiVariant};
use zeroshot_tui::protocol::ClusterSummary;
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

fn cluster_summary(id: &str, state: &str) -> ClusterSummary {
    ClusterSummary {
        id: id.to_string(),
        state: state.to_string(),
        provider: None,
        created_at: 0,
        agent_count: 0,
        message_count: 0,
        cwd: None,
    }
}

#[test]
fn disruptive_render_empty_radar() {
    let backend = TestBackend::new(60, 12);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let mut state = AppState::default();
    state.ui_variant = UiVariant::Disruptive;

    terminal
        .draw(|frame| ui::render(frame, &state))
        .expect("draw");

    let content = buffer_text(terminal.backend().buffer());
    assert!(content.contains("Fleet Radar"));
    assert!(content.contains("No clusters yet."));
    assert!(content.contains("Type an intent in the spine to start a cluster."));
    assert!(!content.contains("ID"));
    assert!(!content.contains("STATE"));
    assert!(!content.contains("ZEROSHOT"));
}

#[test]
fn disruptive_render_radar_three_states() {
    let backend = TestBackend::new(80, 16);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let mut state = AppState::default();
    state.ui_variant = UiVariant::Disruptive;
    state.now_ms = 10_000;
    state.fleet_radar.set_clusters(
        vec![
            cluster_summary("run-1", "running"),
            cluster_summary("done-1", "done"),
            cluster_summary("err-1", "error"),
        ],
        state.now_ms,
    );

    terminal
        .draw(|frame| ui::render(frame, &state))
        .expect("draw");

    let content = buffer_text(terminal.backend().buffer());
    assert!(content.contains("run-1"));
    assert!(content.contains("done-1"));
    assert!(content.contains("err-1"));
    assert!(!content.contains("ID"));
    assert!(!content.contains("STATE"));
}
