use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::{
    AppState, BackendStatus, FocusTarget, ScreenId, SpineHint, SpineHintTone, UiVariant,
};
use crate::screens::{agent, agent_microscope, cluster, cluster_canvas, monitor, radar};
use crate::ui::widgets::{command_bar, scrub_bar, spine, toast};

pub mod launcher;
pub mod scene;
pub mod shared;
pub mod theme;
pub mod widgets;

const DISRUPTIVE_SPINE_HINT: &str = "/guide /nudge /interrupt /pin  /  i  ?  Tab  Esc  Enter";

pub fn render(frame: &mut Frame<'_>, state: &AppState) {
    if matches!(state.ui_variant, UiVariant::Disruptive) {
        render_disruptive(frame, state);
        return;
    }

    let size = frame.area();
    let [header_area, content_area, status_area] = Layout::vertical([
        Constraint::Length(1), // header
        Constraint::Min(1),    // content
        Constraint::Length(1), // status bar / command bar
    ])
    .areas(size);

    render_header(frame, header_area, state);

    match state.active_screen() {
        ScreenId::Launcher | ScreenId::IntentConsole => launcher::render(
            frame,
            content_area,
            &state.launcher,
            state.provider_override.as_deref(),
            &state.monitor.clusters,
        ),
        ScreenId::Monitor | ScreenId::FleetRadar => monitor::render(
            frame,
            content_area,
            &state.monitor,
            &state.metrics,
            state.now_ms,
        ),
        ScreenId::Cluster { id } => {
            if let Some(cluster_state) = state.clusters.get(id) {
                let metrics = state.metrics.get(id);
                cluster::render(frame, content_area, cluster_state, metrics);
            } else {
                let default_state = cluster::State::default();
                cluster::render(frame, content_area, &default_state, None);
            }
        }
        ScreenId::ClusterCanvas { id } => {
            let cluster_state = state.clusters.get(id);
            let canvas_state = state.cluster_canvases.get(id);
            cluster_canvas::render(
                frame,
                content_area,
                cluster_canvas::RenderContext {
                    cluster_id: id,
                    cluster_state,
                    canvas_state,
                    time_cursor: &state.time_cursor,
                    anim_clock: &state.anim_clock,
                    pinned_target: state.pinned_target.as_ref(),
                },
            );
        }
        ScreenId::Agent {
            cluster_id,
            agent_id,
        } => {
            let key = crate::app::AgentKey::new(cluster_id.clone(), agent_id.clone());
            if let Some(agent_state) = state.agents.get(&key) {
                agent::render(frame, content_area, agent_state, cluster_id, agent_id);
            } else {
                let default_state = agent::State::default();
                agent::render(frame, content_area, &default_state, cluster_id, agent_id);
            }
        }
        ScreenId::AgentMicroscope {
            cluster_id,
            agent_id,
        } => {
            let key = crate::app::AgentKey::new(cluster_id.clone(), agent_id.clone());
            let microscope_state = state.agent_microscopes.get(&key);
            let cluster_state = state.clusters.get(cluster_id);
            agent_microscope::render(
                frame,
                content_area,
                cluster_id,
                agent_id,
                cluster_state.map(|state| &state.timeline_time),
                microscope_state,
                &state.time_cursor,
            );
        }
    }

    // Status bar: if command bar active, show command input; otherwise show hints + toast
    let allow_command_bar = !matches!(
        state.active_screen(),
        ScreenId::Launcher | ScreenId::IntentConsole
    );
    if state.command_bar.active {
        command_bar::render(frame, status_area, &state.command_bar, allow_command_bar);
        command_bar::set_cursor(frame, status_area, &state.command_bar);
    } else {
        render_status_bar(frame, status_area, state);
    }
}

fn render_disruptive(frame: &mut Frame<'_>, state: &AppState) {
    let size = frame.area();
    let (canvas_area, scrub_area, spine_area) = if size.height >= 4 {
        let [canvas_area, scrub_area, spine_area] = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(2),
        ])
        .areas(size);
        (canvas_area, Some(scrub_area), spine_area)
    } else {
        let [canvas_area, spine_area] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).areas(size);
        (canvas_area, None, spine_area)
    };

    match state.active_screen() {
        ScreenId::FleetRadar | ScreenId::Launcher | ScreenId::IntentConsole | ScreenId::Monitor => {
            let pinned_cluster = match state.pinned_target.as_ref() {
                Some(FocusTarget::Cluster { id }) => Some(id.as_str()),
                _ => None,
            };
            radar::render(
                frame,
                canvas_area,
                &state.fleet_radar,
                &state.camera,
                state.now_ms,
                &state.anim_clock,
                pinned_cluster,
            );
        }
        ScreenId::ClusterCanvas { id } => {
            let cluster_state = state.clusters.get(id);
            let canvas_state = state.cluster_canvases.get(id);
            cluster_canvas::render(
                frame,
                canvas_area,
                cluster_canvas::RenderContext {
                    cluster_id: id,
                    cluster_state,
                    canvas_state,
                    time_cursor: &state.time_cursor,
                    anim_clock: &state.anim_clock,
                    pinned_target: state.pinned_target.as_ref(),
                },
            );
        }
        ScreenId::Cluster { id } => {
            if let Some(cluster_state) = state.clusters.get(id) {
                let metrics = state.metrics.get(id);
                cluster::render(frame, canvas_area, cluster_state, metrics);
            } else {
                let default_state = cluster::State::default();
                cluster::render(frame, canvas_area, &default_state, None);
            }
        }
        ScreenId::Agent {
            cluster_id,
            agent_id,
        } => {
            let key = crate::app::AgentKey::new(cluster_id.clone(), agent_id.clone());
            if let Some(agent_state) = state.agents.get(&key) {
                agent::render(frame, canvas_area, agent_state, cluster_id, agent_id);
            } else {
                let default_state = agent::State::default();
                agent::render(frame, canvas_area, &default_state, cluster_id, agent_id);
            }
        }
        ScreenId::AgentMicroscope {
            cluster_id,
            agent_id,
        } => {
            let key = crate::app::AgentKey::new(cluster_id.clone(), agent_id.clone());
            let microscope_state = state.agent_microscopes.get(&key);
            let cluster_state = state.clusters.get(cluster_id);
            agent_microscope::render(
                frame,
                canvas_area,
                cluster_id,
                agent_id,
                cluster_state.map(|state| &state.timeline_time),
                microscope_state,
                &state.time_cursor,
            );
        }
    }

    if let Some(scrub_area) = scrub_area {
        let scrub_state = match state.active_screen() {
            ScreenId::ClusterCanvas { id } => Some(scrub_bar::ScrubBarState {
                time_cursor: &state.time_cursor,
                logs: state.clusters.get(id).map(|entry| &entry.logs_time),
                agent_id: None,
            }),
            ScreenId::AgentMicroscope {
                cluster_id,
                agent_id,
            } => Some(scrub_bar::ScrubBarState {
                time_cursor: &state.time_cursor,
                logs: state
                    .agent_microscopes
                    .get(&crate::app::AgentKey::new(
                        cluster_id.clone(),
                        agent_id.clone(),
                    ))
                    .map(|entry| &entry.logs_time),
                agent_id: None,
            }),
            _ => None,
        };
        if let Some(scrub_state) = scrub_state {
            scrub_bar::render(frame, scrub_area, scrub_state);
        }
    }

    let mut spine_state = state.spine.clone();
    if let Some(toast_state) = state.toast.as_ref() {
        if let Some((toast_text, _)) = toast::format_inline(Some(toast_state)) {
            spine_state.hint = SpineHint::from_toast(toast_text, toast_state.level.clone());
        }
    } else if spine_state.hint.is_empty() {
        if let Some(hint) = backend_status_hint(&state.backend_status) {
            spine_state.hint = hint;
        } else {
            spine_state.hint = SpineHint::new(DISRUPTIVE_SPINE_HINT, SpineHintTone::Muted);
        }
    }
    spine::render(frame, spine_area, &spine_state);
    spine::set_cursor(frame, spine_area, &spine_state);
}

fn backend_status_hint(status: &BackendStatus) -> Option<SpineHint> {
    match status {
        BackendStatus::Connected => None,
        BackendStatus::Disconnected => Some(SpineHint::new(
            "○ Backend disconnected",
            SpineHintTone::Muted,
        )),
        BackendStatus::Error(_) => Some(SpineHint::new("✗ Backend error", SpineHintTone::Error)),
        BackendStatus::Exited(_) => Some(SpineHint::new("✗ Backend exited", SpineHintTone::Error)),
    }
}

fn render_header(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let screen = state.active_screen();
    let breadcrumb = screen_breadcrumb(screen);

    let (status_dot, status_style) = match &state.backend_status {
        BackendStatus::Connected => ("●", theme::backend_connected_style()),
        BackendStatus::Disconnected => ("○", theme::backend_error_style()),
        BackendStatus::Error(_) => ("✗", theme::backend_error_style()),
        BackendStatus::Exited(_) => ("✗", theme::backend_error_style()),
    };

    let provider_label = state.provider_override.as_deref().unwrap_or("default");

    // Build left side
    let left = Line::from(vec![
        Span::styled("◆ ZEROSHOT", theme::logo_style()),
        Span::raw("  "),
        Span::styled(breadcrumb, theme::title_style()),
    ]);

    // Build right side
    let right_text = format!("{status_dot} {provider_label}");
    let right_len = right_text.len() as u16 + 1;
    let right = Line::from(vec![
        Span::styled(status_dot, status_style),
        Span::raw(" "),
        Span::styled(provider_label, theme::dim_style()),
    ]);

    // Render left-aligned header
    let widget = Paragraph::new(left);
    frame.render_widget(widget, area);

    // Render right-aligned status
    if area.width > right_len + 20 {
        let right_area = Rect {
            x: area.x + area.width.saturating_sub(right_len),
            y: area.y,
            width: right_len,
            height: 1,
        };
        let right_widget = Paragraph::new(right).alignment(Alignment::Right);
        frame.render_widget(right_widget, right_area);
    }
}

fn render_status_bar(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let hints = screen_hints(state.active_screen());
    let toast_msg = toast::format_inline(state.toast.as_ref());

    let mut spans = Vec::new();
    spans.push(Span::raw(" "));
    for (i, (key, desc)) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(*key, theme::key_style()));
        spans.push(Span::styled(format!(":{desc}"), theme::key_desc_style()));
    }

    // Calculate space for toast on right side
    if let Some((toast_text, toast_style)) = toast_msg {
        let hints_len: usize = spans.iter().map(|s| s.content.len()).sum();
        let toast_len = toast_text.len() + 2;
        let available = area.width as usize;
        if hints_len + toast_len + 4 < available {
            let gap = available.saturating_sub(hints_len + toast_len + 1);
            spans.push(Span::raw(" ".repeat(gap)));
            spans.push(Span::styled(toast_text, toast_style));
        }
    }

    let widget = Paragraph::new(Line::from(spans));
    frame.render_widget(widget, area);
}

fn screen_breadcrumb(screen: &ScreenId) -> String {
    match screen {
        ScreenId::Launcher => "Launcher".to_string(),
        ScreenId::Monitor => "Monitor".to_string(),
        ScreenId::IntentConsole => "Intent Console".to_string(),
        ScreenId::FleetRadar => "Fleet Radar".to_string(),
        ScreenId::Cluster { id } => format!("Monitor > {}", truncate_id(id)),
        ScreenId::ClusterCanvas { id } => format!("Fleet Radar > {}", truncate_id(id)),
        ScreenId::Agent {
            cluster_id,
            agent_id,
        } => format!("Monitor > {} > {}", truncate_id(cluster_id), agent_id),
        ScreenId::AgentMicroscope {
            cluster_id,
            agent_id,
        } => format!("Fleet Radar > {} > {}", truncate_id(cluster_id), agent_id),
    }
}

fn truncate_id(id: &str) -> String {
    const LIMIT: usize = 16;
    let mut iter = id.chars();
    let mut out = String::new();
    for _ in 0..LIMIT {
        match iter.next() {
            Some(ch) => out.push(ch),
            None => return id.to_string(),
        }
    }
    out
}

fn screen_hints(screen: &ScreenId) -> Vec<(&'static str, &'static str)> {
    match screen {
        ScreenId::Launcher => vec![("Enter", "start"), ("/", "commands"), ("Ctrl+C", "quit")],
        ScreenId::IntentConsole => vec![("i", "intent"), ("/", "commands"), ("Esc", "back")],
        ScreenId::Monitor => vec![
            ("j/k", "navigate"),
            ("Enter", "open"),
            ("/", "commands"),
            ("Esc", "back"),
        ],
        ScreenId::FleetRadar => vec![
            ("h/j/k/l", "select"),
            ("g/G", "center"),
            ("Enter", "zoom"),
            ("/", "commands"),
            ("Esc", "back"),
        ],
        ScreenId::Cluster { .. } => vec![
            ("Tab", "pane"),
            ("j/k", "scroll"),
            ("Enter", "agent"),
            ("Esc", "back"),
        ],
        ScreenId::ClusterCanvas { .. } => vec![
            ("h/j/k/l", "focus"),
            ("Shift+h/j/k/l", "fast"),
            ("Enter", "zoom"),
            ("Esc", "back"),
        ],
        ScreenId::Agent { .. } => vec![("Enter", "send"), ("j/k", "scroll"), ("Esc", "back")],
        ScreenId::AgentMicroscope { .. } => vec![("Esc", "back")],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::widgets::test_utils::line_text;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::Terminal;

    fn buffer_contains(buffer: &Buffer, needle: &str) -> bool {
        for y in 0..buffer.area.height {
            if line_text(buffer, y).contains(needle) {
                return true;
            }
        }
        false
    }

    #[test]
    fn disruptive_spine_shows_backend_disconnected() {
        let backend = TestBackend::new(80, 8);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut state = AppState::default();
        state.ui_variant = UiVariant::Disruptive;
        state.screen_stack = vec![ScreenId::IntentConsole, ScreenId::FleetRadar];
        state.backend_status = BackendStatus::Disconnected;
        state.spine.hint = SpineHint::empty();
        state.toast = None;

        terminal
            .draw(|frame| {
                render(frame, &state);
            })
            .expect("draw");

        let buffer = terminal.backend().buffer();
        assert!(buffer_contains(buffer, "Backend disconnected"));
    }
}
