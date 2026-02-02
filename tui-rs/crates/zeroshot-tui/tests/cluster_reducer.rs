use zeroshot_tui::protocol::{ClusterLogLine, TimelineEvent};
use zeroshot_tui::screens::cluster::{self, ClusterPane, FocusDirection};

fn log_line(id: usize, agent: Option<&str>, role: Option<&str>) -> ClusterLogLine {
    ClusterLogLine {
        id: format!("log-{id}"),
        timestamp: id as i64,
        text: format!("line-{id}"),
        agent: agent.map(|value| value.to_string()),
        role: role.map(|value| value.to_string()),
        sender: None,
    }
}

fn timeline_event(id: usize) -> TimelineEvent {
    TimelineEvent {
        id: format!("event-{id}"),
        timestamp: id as i64,
        topic: format!("topic-{id}"),
        label: format!("label-{id}"),
        approved: None,
        sender: None,
    }
}

#[test]
fn log_buffer_bounds_and_dropped_count() {
    let mut state = cluster::State::default();
    state.push_log_lines(vec![log_line(0, None, None)], Some(3));
    assert_eq!(state.logs.len(), 2);
    let first = state.logs.items.front().expect("expected synthetic line");
    assert!(first.text.contains("dropped 3"));

    let mut state = cluster::State::default();
    let lines: Vec<_> = (0..(cluster::MAX_LOG_LINES + 5))
        .map(|id| log_line(id, None, None))
        .collect();
    state.push_log_lines(lines, None);
    assert_eq!(state.logs.len(), cluster::MAX_LOG_LINES);
    let first = state.logs.items.front().expect("expected log line");
    assert_eq!(first.id, "log-5");
}

#[test]
fn timeline_buffer_bounds() {
    let mut state = cluster::State::default();
    let events: Vec<_> = (0..(cluster::MAX_TIMELINE_EVENTS + 3))
        .map(timeline_event)
        .collect();
    state.push_timeline_events(events);
    assert_eq!(state.timeline.len(), cluster::MAX_TIMELINE_EVENTS);
    let first = state.timeline.items.front().expect("expected event");
    assert_eq!(first.id, "event-3");
}

#[test]
fn focus_cycles_and_activate_uses_selected_agent() {
    let mut state = cluster::State::default();
    assert_eq!(state.focus, ClusterPane::Topology);
    state.cycle_focus(FocusDirection::Next);
    assert_eq!(state.focus, ClusterPane::Logs);
    state.cycle_focus(FocusDirection::Next);
    assert_eq!(state.focus, ClusterPane::Timeline);
    state.cycle_focus(FocusDirection::Next);
    assert_eq!(state.focus, ClusterPane::Agents);
    state.cycle_focus(FocusDirection::Prev);
    assert_eq!(state.focus, ClusterPane::Timeline);

    state.focus = ClusterPane::Agents;
    state.push_log_lines(vec![log_line(1, Some("agent-a"), Some("role-a"))], None);
    state.push_log_lines(vec![log_line(2, Some("agent-b"), Some("role-b"))], None);
    state.move_focused(1);
    assert_eq!(state.activate_focused(), Some("agent-b".to_string()));

    state.push_log_lines(vec![log_line(3, Some("agent-c"), Some("role-c"))], None);
    assert_eq!(state.activate_focused(), Some("agent-b".to_string()));
}

#[test]
fn scroll_offset_grows_when_new_lines_arrive() {
    let mut state = cluster::State::default();
    state.focus = ClusterPane::Logs;
    state.push_log_lines(vec![log_line(0, None, None), log_line(1, None, None)], None);
    state.move_focused(-1);
    assert_eq!(state.logs.scroll_offset, 1);
    state.push_log_lines(vec![log_line(2, None, None)], None);
    assert_eq!(state.logs.scroll_offset, 2);
}
