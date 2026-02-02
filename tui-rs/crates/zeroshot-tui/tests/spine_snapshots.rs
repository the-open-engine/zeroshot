mod ui_snapshot_helpers;

use ui_snapshot_helpers::render_to_text;

use zeroshot_tui::app::{SpineCompletion, SpineHint, SpineHintTone, SpineMode, SpineState};
use zeroshot_tui::ui::widgets::spine;

const WIDTH: u16 = 72;
const HEIGHT: u16 = 3;

fn render_spine(state: &SpineState) -> String {
    render_to_text(WIDTH, HEIGHT, |frame| {
        let area = frame.area();
        spine::render(frame, area, state);
    })
}

#[test]
fn spine_intent_mode_with_hint() {
    let mut state = SpineState::default();
    state.hint = SpineHint::new("Ready", SpineHintTone::Info);

    let content = render_spine(&state);
    assert!(content.contains("Intent"));
    assert!(content.contains("Type intent..."));
    assert!(content.contains("Ready"));
}

#[test]
fn spine_command_mode_with_completion() {
    let mut state = SpineState::default();
    state.mode = SpineMode::Command;
    state.input.input = "pin".to_string();
    state.input.cursor = state.input.input.len();
    state.completion = Some(SpineCompletion {
        candidates: vec!["cluster-1".to_string()],
        selected: 0,
        ghost: " cluster-1".to_string(),
    });
    state.hint = SpineHint::new("Pin focus", SpineHintTone::Muted);

    let content = render_spine(&state);
    assert!(content.contains("Command"));
    assert!(content.contains("/pin cluster-1"));
    assert!(content.contains("Pin focus"));
}

#[test]
fn spine_whisper_cluster_mode() {
    let mut state = SpineState::default();
    state.mode = SpineMode::WhisperCluster;
    state.input.input = "cluster-7".to_string();
    state.input.cursor = state.input.input.len();
    state.hint = SpineHint::new("Whisper to cluster", SpineHintTone::Info);

    let content = render_spine(&state);
    assert!(content.contains("Whisper Cluster"));
    assert!(content.contains("cluster-7"));
    assert!(content.contains("Whisper to cluster"));
}

#[test]
fn spine_whisper_agent_mode() {
    let mut state = SpineState::default();
    state.mode = SpineMode::WhisperAgent;
    state.input.input = "agent-2".to_string();
    state.input.cursor = state.input.input.len();
    state.hint = SpineHint::new("Whisper to agent", SpineHintTone::Info);

    let content = render_spine(&state);
    assert!(content.contains("Whisper Agent"));
    assert!(content.contains("agent-2"));
    assert!(content.contains("Whisper to agent"));
}
