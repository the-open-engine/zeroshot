mod ui_snapshot_helpers;

use ui_snapshot_helpers::render_to_text;

use zeroshot_tui::app::{TimeCursor, TimeCursorMode};
use zeroshot_tui::protocol::ClusterLogLine;
use zeroshot_tui::ui::shared::TimeIndexedBuffer;
use zeroshot_tui::ui::widgets::scrub_bar::{self, ScrubBarState};

fn sample_logs(timestamps: &[i64]) -> TimeIndexedBuffer<ClusterLogLine> {
    let mut buffer = TimeIndexedBuffer::new(64);
    let lines = timestamps.iter().map(|ts| ClusterLogLine {
        id: format!("log-{ts}"),
        timestamp: *ts,
        text: "event".to_string(),
        agent: None,
        role: None,
        sender: None,
    });
    buffer.push_many(lines);
    buffer
}

#[test]
fn scrub_bar_snapshot_live_mode() {
    let logs = sample_logs(&[100, 200, 300]);
    let cursor = TimeCursor {
        mode: TimeCursorMode::Live,
        t_ms: 300,
        window_ms: 300,
    };

    let content = render_to_text(40, 1, |frame| {
        let area = frame.area();
        scrub_bar::render(
            frame,
            area,
            ScrubBarState {
                time_cursor: &cursor,
                logs: Some(&logs),
                agent_id: None,
            },
        );
    });

    assert!(content.contains("LIVE"));
    assert!(content.contains("|"));
}

#[test]
fn scrub_bar_snapshot_scrub_mode() {
    let logs = sample_logs(&[100, 200, 300]);
    let cursor = TimeCursor {
        mode: TimeCursorMode::Scrub,
        t_ms: 150,
        window_ms: 300,
    };

    let content = render_to_text(40, 1, |frame| {
        let area = frame.area();
        scrub_bar::render(
            frame,
            area,
            ScrubBarState {
                time_cursor: &cursor,
                logs: Some(&logs),
                agent_id: None,
            },
        );
    });

    assert!(content.contains("SCRUB"));
    assert!(content.contains("*") || content.contains("^"));
}
