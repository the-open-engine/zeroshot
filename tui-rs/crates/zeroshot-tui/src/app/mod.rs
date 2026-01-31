use std::collections::HashMap;

use crate::backend::{BackendExit, BackendNotification};
use crate::screens::{agent, cluster, launcher, monitor};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AgentKey {
    pub cluster_id: String,
    pub agent_id: String,
}

impl AgentKey {
    pub fn new(cluster_id: impl Into<String>, agent_id: impl Into<String>) -> Self {
        Self {
            cluster_id: cluster_id.into(),
            agent_id: agent_id.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ScreenId {
    Launcher,
    Monitor,
    Cluster { id: String },
    Agent { cluster_id: String, agent_id: String },
}

impl ScreenId {
    pub fn title(&self) -> String {
        match self {
            ScreenId::Launcher => "Launcher".to_string(),
            ScreenId::Monitor => "Monitor".to_string(),
            ScreenId::Cluster { id } => format!("Cluster {id}"),
            ScreenId::Agent {
                cluster_id,
                agent_id,
            } => format!("Agent {agent_id} @ {cluster_id}"),
        }
    }
}

#[derive(Debug, Clone)]
pub enum BackendStatus {
    Disconnected,
    Connected,
    Error(String),
    Exited(BackendExit),
}

#[derive(Debug, Clone)]
pub struct AppState {
    pub screen_stack: Vec<ScreenId>,
    pub launcher: launcher::State,
    pub monitor: monitor::State,
    pub clusters: HashMap<String, cluster::State>,
    pub agents: HashMap<AgentKey, agent::State>,
    pub last_size: Option<(u16, u16)>,
    pub tick_count: u64,
    pub now_ms: i64,
    pub should_quit: bool,
    pub backend_status: BackendStatus,
    pub last_error: Option<String>,
    pub provider_override: Option<String>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            screen_stack: vec![ScreenId::Launcher],
            launcher: launcher::State::default(),
            monitor: monitor::State::default(),
            clusters: HashMap::new(),
            agents: HashMap::new(),
            last_size: None,
            tick_count: 0,
            now_ms: 0,
            should_quit: false,
            backend_status: BackendStatus::Disconnected,
            last_error: None,
            provider_override: None,
        }
    }
}

impl AppState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn active_screen(&self) -> &ScreenId {
        self.screen_stack
            .last()
            .unwrap_or(&ScreenId::Launcher)
    }

    fn ensure_screen_state(&mut self, screen: &ScreenId) {
        match screen {
            ScreenId::Launcher | ScreenId::Monitor => {}
            ScreenId::Cluster { id } => {
                self.clusters
                    .entry(id.clone())
                    .or_insert_with(cluster::State::default);
            }
            ScreenId::Agent {
                cluster_id,
                agent_id,
            } => {
                let key = AgentKey::new(cluster_id.clone(), agent_id.clone());
                self.agents
                    .entry(key)
                    .or_insert_with(agent::State::default);
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NavigationAction {
    Push(ScreenId),
    Pop,
    ReplaceTop(ScreenId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScreenAction {
    Launcher(launcher::Action),
    Monitor(monitor::Action),
    Cluster {
        id: String,
        action: cluster::Action,
    },
    Agent {
        cluster_id: String,
        agent_id: String,
        action: agent::Action,
    },
}

#[derive(Debug, Clone)]
pub enum BackendAction {
    Connected,
    ConnectionFailed(String),
    BackendExited(BackendExit),
    Notification(BackendNotification),
    ClustersListed(Vec<crate::protocol::ClusterSummary>),
    ClusterSummary {
        summary: crate::protocol::ClusterSummary,
    },
    SubscribedClusterLogs {
        cluster_id: String,
        subscription_id: String,
    },
    SubscribedClusterTimeline {
        cluster_id: String,
        subscription_id: String,
    },
    StartClusterResult {
        cluster_id: String,
    },
    Error(String),
}

#[derive(Debug, Clone)]
pub enum Action {
    Tick { now_ms: i64 },
    Resize { width: u16, height: u16 },
    Quit,
    Navigate(NavigationAction),
    Screen(ScreenAction),
    Backend(BackendAction),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Backend(BackendRequest),
    Command(CommandRequest),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendRequest {
    ListClusters,
    GetClusterSummary { cluster_id: String },
    SubscribeClusterLogs { cluster_id: String },
    SubscribeClusterTimeline { cluster_id: String },
    StartClusterFromText {
        text: String,
        provider_override: Option<String>,
    },
    StartClusterFromIssue {
        reference: String,
        provider_override: Option<String>,
    },
    SendGuidanceToCluster { cluster_id: String, message: String },
    SendGuidanceToAgent {
        cluster_id: String,
        agent_id: String,
        message: String,
    },
    Unsubscribe { subscription_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandRequest {
    SubmitRaw { raw: String },
}

pub fn update(mut state: AppState, action: Action) -> (AppState, Vec<Effect>) {
    let mut effects = Vec::new();
    match action {
        Action::Tick { now_ms } => {
            state.tick_count = state.tick_count.saturating_add(1);
            state.now_ms = now_ms;
            let should_poll = matches!(state.active_screen(), ScreenId::Monitor)
                && state.monitor.poll_due(now_ms);
            if should_poll {
                state.monitor.mark_polled(now_ms);
                effects.push(Effect::Backend(BackendRequest::ListClusters));
            }
        }
        Action::Resize { width, height } => {
            state.last_size = Some((width, height));
        }
        Action::Quit => {
            state.should_quit = true;
        }
        Action::Navigate(nav) => {
            apply_navigation(&mut state, nav, &mut effects);
        }
        Action::Screen(screen_action) => {
            handle_screen_action(&mut state, screen_action, &mut effects);
        }
        Action::Backend(backend_action) => {
            handle_backend_action(&mut state, backend_action, &mut effects);
        }
    }

    (state, effects)
}

fn apply_navigation(state: &mut AppState, nav: NavigationAction, effects: &mut Vec<Effect>) {
    match nav {
        NavigationAction::Push(screen) => {
            state.ensure_screen_state(&screen);
            if matches!(screen, ScreenId::Monitor) {
                state.monitor.mark_polled(state.now_ms);
            }
            state.screen_stack.push(screen.clone());
            queue_navigation_effects(&screen, effects);
        }
        NavigationAction::Pop => {
            if state.screen_stack.len() > 1 {
                state.screen_stack.pop();
            }
        }
        NavigationAction::ReplaceTop(screen) => {
            state.ensure_screen_state(&screen);
            if matches!(screen, ScreenId::Monitor) {
                state.monitor.mark_polled(state.now_ms);
            }
            if state.screen_stack.is_empty() {
                state.screen_stack.push(screen.clone());
            } else {
                let top = state.screen_stack.len() - 1;
                state.screen_stack[top] = screen.clone();
            }
            queue_navigation_effects(&screen, effects);
        }
    }
}

fn queue_navigation_effects(screen: &ScreenId, effects: &mut Vec<Effect>) {
    match screen {
        ScreenId::Monitor => {
            effects.push(Effect::Backend(BackendRequest::ListClusters));
        }
        ScreenId::Cluster { id } => {
            effects.push(Effect::Backend(BackendRequest::GetClusterSummary {
                cluster_id: id.clone(),
            }));
            effects.push(Effect::Backend(BackendRequest::SubscribeClusterLogs {
                cluster_id: id.clone(),
            }));
            effects.push(Effect::Backend(BackendRequest::SubscribeClusterTimeline {
                cluster_id: id.clone(),
            }));
        }
        ScreenId::Agent { .. } | ScreenId::Launcher => {}
    }
}

fn handle_screen_action(state: &mut AppState, action: ScreenAction, effects: &mut Vec<Effect>) {
    match action {
        ScreenAction::Launcher(action) => handle_launcher_action(state, action, effects),
        ScreenAction::Monitor(action) => handle_monitor_action(state, action, effects),
        ScreenAction::Cluster { id, action } => handle_cluster_action(state, id, action, effects),
        ScreenAction::Agent {
            cluster_id,
            agent_id,
            action,
        } => handle_agent_action(state, cluster_id, agent_id, action),
    }
}

fn handle_launcher_action(state: &mut AppState, action: launcher::Action, effects: &mut Vec<Effect>) {
    match action {
        launcher::Action::Submit => {
            let trimmed = state.launcher.input.trim();
            if trimmed.is_empty() {
                state.last_error = Some("Enter text to start a cluster.".to_string());
                return;
            }

            state.last_error = None;
            if trimmed.starts_with('/') {
                effects.push(Effect::Command(CommandRequest::SubmitRaw {
                    raw: trimmed.to_string(),
                }));
            } else {
                effects.push(Effect::Backend(BackendRequest::StartClusterFromText {
                    text: trimmed.to_string(),
                    provider_override: state.provider_override.clone(),
                }));
            }
        }
        launcher::Action::InsertChar(ch) => {
            state.launcher.insert_char(ch);
            state.last_error = None;
        }
        launcher::Action::Backspace => {
            state.launcher.backspace();
            state.last_error = None;
        }
        launcher::Action::Delete => {
            state.launcher.delete();
            state.last_error = None;
        }
        launcher::Action::MoveCursorLeft => {
            state.launcher.move_left();
        }
        launcher::Action::MoveCursorRight => {
            state.launcher.move_right();
        }
        launcher::Action::MoveCursorHome => {
            state.launcher.move_home();
        }
        launcher::Action::MoveCursorEnd => {
            state.launcher.move_end();
        }
    }
}

fn handle_monitor_action(state: &mut AppState, action: monitor::Action, effects: &mut Vec<Effect>) {
    match action {
        monitor::Action::MoveSelection(delta) => {
            state.monitor.move_selection(delta);
        }
        monitor::Action::OpenSelected => {
            if let Some(cluster_id) = state.monitor.selected_cluster_id() {
                apply_navigation(
                    state,
                    NavigationAction::Push(ScreenId::Cluster { id: cluster_id }),
                    effects,
                );
            }
        }
    }
}

fn handle_cluster_action(
    state: &mut AppState,
    id: String,
    action: cluster::Action,
    effects: &mut Vec<Effect>,
) {
    let entry = state
        .clusters
        .entry(id.clone())
        .or_insert_with(cluster::State::default);
    match action {
        cluster::Action::CycleFocus(direction) => {
            entry.cycle_focus(direction);
        }
        cluster::Action::OpenAgent(agent_id) => {
            apply_navigation(
                state,
                NavigationAction::Push(ScreenId::Agent {
                    cluster_id: id,
                    agent_id,
                }),
                effects,
            );
        }
    }
}

fn handle_agent_action(
    state: &mut AppState,
    cluster_id: String,
    agent_id: String,
    action: agent::Action,
) {
    let key = AgentKey::new(cluster_id, agent_id);
    let entry = state
        .agents
        .entry(key)
        .or_insert_with(agent::State::default);
    match action {
        agent::Action::Noop => {
            entry.bump();
        }
    }
}

fn handle_backend_action(state: &mut AppState, action: BackendAction, effects: &mut Vec<Effect>) {
    match action {
        BackendAction::Connected => handle_backend_connected(state),
        BackendAction::ConnectionFailed(message) => handle_backend_connection_failed(state, message),
        BackendAction::BackendExited(exit) => handle_backend_exited(state, exit),
        BackendAction::Notification(notification) => handle_backend_notification(state, notification),
        BackendAction::ClustersListed(clusters) => handle_clusters_listed(state, clusters),
        BackendAction::ClusterSummary { summary } => handle_cluster_summary(state, summary),
        BackendAction::SubscribedClusterLogs {
            cluster_id,
            subscription_id,
        } => handle_cluster_log_subscription(state, cluster_id, subscription_id),
        BackendAction::SubscribedClusterTimeline {
            cluster_id,
            subscription_id,
        } => handle_cluster_timeline_subscription(state, cluster_id, subscription_id),
        BackendAction::StartClusterResult { cluster_id } => {
            handle_start_cluster_result(state, cluster_id, effects)
        }
        BackendAction::Error(message) => handle_backend_error(state, message),
    }
}

fn handle_backend_connected(state: &mut AppState) {
    state.backend_status = BackendStatus::Connected;
}

fn handle_backend_connection_failed(state: &mut AppState, message: String) {
    state.backend_status = BackendStatus::Error(message.clone());
    state.last_error = Some(message);
}

fn handle_backend_exited(state: &mut AppState, exit: BackendExit) {
    state.backend_status = BackendStatus::Exited(exit.clone());
    state.last_error = Some(exit.message);
}

fn handle_backend_notification(state: &mut AppState, notification: BackendNotification) {
    match notification {
        BackendNotification::ClusterLogLines(params) => {
            let entry = state
                .clusters
                .entry(params.cluster_id)
                .or_insert_with(cluster::State::default);
            entry.push_log_lines(params.lines);
        }
        BackendNotification::ClusterTimelineEvents(params) => {
            let entry = state
                .clusters
                .entry(params.cluster_id)
                .or_insert_with(cluster::State::default);
            entry.push_timeline_events(params.events);
        }
        BackendNotification::Unknown { method, .. } => {
            state.last_error = Some(format!("Unhandled backend notification: {method}"));
        }
    }
}

fn handle_clusters_listed(
    state: &mut AppState,
    clusters: Vec<crate::protocol::ClusterSummary>,
) {
    state.monitor.set_clusters(clusters, state.now_ms);
}

fn handle_cluster_summary(state: &mut AppState, summary: crate::protocol::ClusterSummary) {
    let entry = state
        .clusters
        .entry(summary.id.clone())
        .or_insert_with(cluster::State::default);
    entry.summary = Some(summary);
}

fn handle_cluster_log_subscription(
    state: &mut AppState,
    cluster_id: String,
    subscription_id: String,
) {
    let entry = state
        .clusters
        .entry(cluster_id)
        .or_insert_with(cluster::State::default);
    entry.log_subscription = Some(subscription_id);
}

fn handle_cluster_timeline_subscription(
    state: &mut AppState,
    cluster_id: String,
    subscription_id: String,
) {
    let entry = state
        .clusters
        .entry(cluster_id)
        .or_insert_with(cluster::State::default);
    entry.timeline_subscription = Some(subscription_id);
}

fn handle_start_cluster_result(
    state: &mut AppState,
    cluster_id: String,
    effects: &mut Vec<Effect>,
) {
    state.launcher.clear();
    apply_navigation(
        state,
        NavigationAction::Push(ScreenId::Cluster { id: cluster_id }),
        effects,
    );
}

fn handle_backend_error(state: &mut AppState, message: String) {
    state.last_error = Some(message);
}
