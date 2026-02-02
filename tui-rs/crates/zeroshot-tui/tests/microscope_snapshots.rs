mod ui_snapshot_helpers;

use ui_snapshot_helpers::render_to_text;

use zeroshot_tui::app::{agent_microscope, TimeCursor, TimeCursorMode};
use zeroshot_tui::protocol::{ClusterLogLine, TimelineEvent};
use zeroshot_tui::screens::agent_microscope as microscope_screen;
use zeroshot_tui::ui::shared::TimeIndexedBuffer;

fn sample_logs() -> TimeIndexedBuffer<ClusterLogLine> {
    let mut buffer = TimeIndexedBuffer::new(32);
    buffer.push_many(vec![
        ClusterLogLine {
            id: "log-1".to_string(),
            timestamp: 100,
            text: "started task".to_string(),
            agent: Some("agent-9".to_string()),
            role: Some("implementation".to_string()),
            sender: Some("agent-9".to_string()),
        },
        ClusterLogLine {
            id: "log-2".to_string(),
            timestamp: 200,
            text: "finished task".to_string(),
            agent: Some("agent-9".to_string()),
            role: Some("implementation".to_string()),
            sender: Some("agent-9".to_string()),
        },
    ]);
    buffer
}

fn sample_timeline() -> TimeIndexedBuffer<TimelineEvent> {
    let mut buffer = TimeIndexedBuffer::new(16);
    buffer.push_many(vec![TimelineEvent {
        id: "evt-1".to_string(),
        timestamp: 150,
        topic: "IMPLEMENTATION_READY".to_string(),
        label: "ready".to_string(),
        approved: None,
        sender: Some("agent-9".to_string()),
    }]);
    buffer
}

fn sample_state() -> agent_microscope::State {
    let mut state = agent_microscope::State::default();
    state.logs_time = sample_logs();
    state.role = Some("implementation".to_string());
    state.status = Some("running".to_string());
    state
}

#[test]
fn microscope_snapshot_live_mode() {
    let microscope_state = sample_state();
    let timeline = sample_timeline();
    let cursor = TimeCursor {
        mode: TimeCursorMode::Live,
        t_ms: 200,
        window_ms: 400,
    };

    let content = render_to_text(80, 18, |frame| {
        let area = frame.area();
        microscope_screen::render(
            frame,
            area,
            "cluster-7",
            "agent-9",
            Some(&timeline),
            Some(&microscope_state),
            &cursor,
        );
    });

    assert!(content.contains("Stream"));
    assert!(content.contains("[LIVE]"));
    assert!(content.contains("Agent: agent-9"));
    assert!(content.contains("Role: implementation"));
    assert!(content.contains("Status: running"));
    assert!(content.contains("Cluster: cluster-7"));
    assert!(content.contains("started task"));
}

#[test]
fn microscope_snapshot_scrub_mode() {
    let microscope_state = sample_state();
    let timeline = sample_timeline();
    let cursor = TimeCursor {
        mode: TimeCursorMode::Scrub,
        t_ms: 200,
        window_ms: 200,
    };

    let content = render_to_text(80, 18, |frame| {
        let area = frame.area();
        microscope_screen::render(
            frame,
            area,
            "cluster-7",
            "agent-9",
            Some(&timeline),
            Some(&microscope_state),
            &cursor,
        );
    });

    assert!(content.contains("Stream"));
    assert!(content.contains("[SCRUB]"));
    assert!(content.contains("Agent: agent-9"));
    assert!(content.contains("Cluster: cluster-7"));
    assert!(content.contains("finished task"));
}
