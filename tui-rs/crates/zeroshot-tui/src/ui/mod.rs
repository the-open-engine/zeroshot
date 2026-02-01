use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::{AppState, BackendStatus, ScreenId, UiVariant};
use crate::screens::{agent, cluster, monitor};
use crate::ui::widgets::{command_bar, toast};

pub mod launcher;
pub mod scene;
pub mod shared;
pub mod theme;
pub mod widgets;

pub fn render(frame: &mut Frame<'_>, state: &AppState) {
    if matches!(state.ui_variant, UiVariant::Disruptive) {
        render_disruptive(frame, state);
        return;
    }

    let size = frame.area();
    let [header_area, content_area, status_area] = Layout::vertical([
        Constraint::Length(1), // header
        Constraint::Min(1),   // content
        Constraint::Length(1), // status bar / command bar
    ])
    .areas(size);

    render_header(frame, header_area, state);

    match state.active_screen() {
        ScreenId::Launcher => launcher::render(
            frame,
            content_area,
            &state.launcher,
            state.provider_override.as_deref(),
            &state.monitor.clusters,
        ),
        ScreenId::Monitor => {
            monitor::render(frame, content_area, &state.monitor, &state.metrics, state.now_ms)
        }
        ScreenId::Cluster { id } => {
            if let Some(cluster_state) = state.clusters.get(id) {
                let metrics = state.metrics.get(id);
                cluster::render(frame, content_area, cluster_state, metrics);
            } else {
                let default_state = cluster::State::default();
                cluster::render(frame, content_area, &default_state, None);
            }
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
    }

    // Status bar: if command bar active, show command input; otherwise show hints + toast
    let allow_command_bar = !matches!(state.active_screen(), ScreenId::Launcher);
    if state.command_bar.active {
        command_bar::render(frame, status_area, &state.command_bar, allow_command_bar);
        command_bar::set_cursor(frame, status_area, &state.command_bar);
    } else {
        render_status_bar(frame, status_area, state);
    }
}

fn render_disruptive(frame: &mut Frame<'_>, _state: &AppState) {
    let size = frame.area();
    let [canvas_area, spine_area] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(3),
    ])
    .areas(size);

    let canvas = Paragraph::new("Canvas").alignment(Alignment::Center).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Disruptive"),
    );
    frame.render_widget(canvas, canvas_area);

    let spine = Paragraph::new("Spine").alignment(Alignment::Left).block(
        Block::default()
            .borders(Borders::TOP)
            .title("Spine"),
    );
    frame.render_widget(spine, spine_area);
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

    let provider_label = state
        .provider_override
        .as_deref()
        .unwrap_or("default");

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
        ScreenId::Cluster { id } => format!("Monitor > {}", truncate_id(id)),
        ScreenId::Agent {
            cluster_id,
            agent_id,
        } => format!(
            "Monitor > {} > {}",
            truncate_id(cluster_id),
            agent_id
        ),
    }
}

fn truncate_id(id: &str) -> &str {
    if id.len() > 16 {
        &id[..16]
    } else {
        id
    }
}

fn screen_hints(screen: &ScreenId) -> Vec<(&'static str, &'static str)> {
    match screen {
        ScreenId::Launcher => vec![
            ("Enter", "start"),
            ("/", "commands"),
            ("Ctrl+C", "quit"),
        ],
        ScreenId::Monitor => vec![
            ("j/k", "navigate"),
            ("Enter", "open"),
            ("/", "commands"),
            ("Esc", "back"),
        ],
        ScreenId::Cluster { .. } => vec![
            ("Tab", "pane"),
            ("j/k", "scroll"),
            ("Enter", "agent"),
            ("Esc", "back"),
        ],
        ScreenId::Agent { .. } => vec![
            ("Enter", "send"),
            ("j/k", "scroll"),
            ("Esc", "back"),
        ],
    }
}
