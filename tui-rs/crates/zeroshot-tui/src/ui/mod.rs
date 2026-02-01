use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::{AppState, BackendStatus, ScreenId};
use crate::screens::{agent, cluster, monitor};
use crate::ui::widgets::{command_bar, toast};

pub mod launcher;
pub mod widgets;

pub fn render(frame: &mut Frame<'_>, state: &AppState) {
    let size = frame.size();
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(3),
            Constraint::Length(3),
        ])
        .split(size);

    render_header(frame, layout[0], state);

    match state.active_screen() {
        ScreenId::Launcher => launcher::render(
            frame,
            layout[1],
            &state.launcher,
            state.provider_override.as_deref(),
        ),
        ScreenId::Monitor => {
            monitor::render(frame, layout[1], &state.monitor, &state.metrics, state.now_ms)
        }
        ScreenId::Cluster { id } => {
            if let Some(cluster_state) = state.clusters.get(id) {
                let metrics = state.metrics.get(id);
                cluster::render(frame, layout[1], cluster_state, metrics);
            } else {
                let default_state = cluster::State::default();
                cluster::render(frame, layout[1], &default_state, None);
            }
        }
        ScreenId::Agent {
            cluster_id,
            agent_id,
        } => {
            let key = crate::app::AgentKey::new(cluster_id.clone(), agent_id.clone());
            if let Some(agent_state) = state.agents.get(&key) {
                agent::render(frame, layout[1], agent_state, cluster_id, agent_id);
            } else {
                let default_state = agent::State::default();
                agent::render(frame, layout[1], &default_state, cluster_id, agent_id);
            }
        }
    }

    toast::render(frame, layout[2], state.toast.as_ref());
    let allow_command_bar = !matches!(state.active_screen(), ScreenId::Launcher);
    command_bar::render(frame, layout[3], &state.command_bar, allow_command_bar);
    command_bar::set_cursor(frame, layout[3], &state.command_bar);
}

fn render_header(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let (backend_label, backend_style) = match &state.backend_status {
        BackendStatus::Disconnected => (
            "backend: disconnected".to_string(),
            Style::default().fg(Color::Red),
        ),
        BackendStatus::Connected => (
            "backend: connected".to_string(),
            Style::default().fg(Color::Green),
        ),
        BackendStatus::Error(message) => (
            format!("backend: error ({message})"),
            Style::default().fg(Color::Red),
        ),
        BackendStatus::Exited(exit) => (
            format!("backend: exited ({})", exit.code.unwrap_or(-1)),
            Style::default().fg(Color::Red),
        ),
    };

    let mut spans = vec![
        Span::styled(
            format!("Screen: {}", state.active_screen().title()),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(backend_label, backend_style),
        Span::raw("  "),
        Span::raw(format!("ticks {}", state.tick_count)),
    ];

    if let Some(error) = &state.last_error {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!("last error: {error}"),
            Style::default().fg(Color::Red),
        ));
    }

    let widget = Paragraph::new(Line::from(spans))
        .block(Block::default().borders(Borders::BOTTOM));
    frame.render_widget(widget, area);
}
