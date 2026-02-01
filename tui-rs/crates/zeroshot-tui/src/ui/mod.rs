use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::{AppState, BackendStatus, ScreenId};
use crate::screens::{agent, cluster, monitor};

pub mod launcher;

pub fn render(frame: &mut Frame<'_>, state: &AppState) {
    let size = frame.size();
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(size);

    render_header(frame, layout[0], state);

    match state.active_screen() {
        ScreenId::Launcher => launcher::render(
            frame,
            layout[1],
            &state.launcher,
            state.provider_override.as_deref(),
        ),
        ScreenId::Monitor => monitor::render(frame, layout[1], &state.monitor, state.now_ms),
        ScreenId::Cluster { id } => {
            if let Some(cluster_state) = state.clusters.get(id) {
                cluster::render(frame, layout[1], cluster_state);
            } else {
                let default_state = cluster::State::default();
                cluster::render(frame, layout[1], &default_state);
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
}

fn render_header(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let backend = match &state.backend_status {
        BackendStatus::Disconnected => "backend: disconnected".to_string(),
        BackendStatus::Connected => "backend: connected".to_string(),
        BackendStatus::Error(message) => format!("backend: error ({message})"),
        BackendStatus::Exited(exit) => {
            format!("backend: exited ({})", exit.code.unwrap_or(-1))
        }
    };

    let mut lines = vec![
        Line::from(format!("Screen: {}", state.active_screen().title())),
        Line::from(format!("{} | ticks: {}", backend, state.tick_count)),
    ];

    if let Some(error) = &state.last_error {
        lines.push(Line::from(format!("last error: {error}")));
    }

    let widget = Paragraph::new(lines).block(Block::default().borders(Borders::ALL));
    frame.render_widget(widget, area);
}
