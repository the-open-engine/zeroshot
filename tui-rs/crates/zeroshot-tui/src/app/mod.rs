use std::collections::{HashMap, HashSet};

use crate::backend::{BackendExit, BackendNotification};
use crate::screens::{agent, cluster, launcher, monitor};
use crate::protocol::ClusterMetrics;
use crate::ui::shared::InputState;

mod spine_hint;
pub use spine_hint::{compute_spine_hint, SpineHint, SpineHintTone};

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
    IntentConsole,
    FleetRadar,
    Cluster { id: String },
    ClusterCanvas { id: String },
    Agent { cluster_id: String, agent_id: String },
    AgentMicroscope { cluster_id: String, agent_id: String },
}

impl ScreenId {
    pub fn title(&self) -> String {
        match self {
            ScreenId::Launcher => "Launcher".to_string(),
            ScreenId::Monitor => "Monitor".to_string(),
            ScreenId::IntentConsole => "Intent Console".to_string(),
            ScreenId::FleetRadar => "Fleet Radar".to_string(),
            ScreenId::Cluster { id } => format!("Cluster {id}"),
            ScreenId::ClusterCanvas { id } => format!("Cluster Canvas {id}"),
            ScreenId::Agent {
                cluster_id,
                agent_id,
            } => format!("Agent {agent_id} @ {cluster_id}"),
            ScreenId::AgentMicroscope {
                cluster_id,
                agent_id,
            } => format!("Agent Microscope {agent_id} @ {cluster_id}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZoomStackContext {
    Root,
    FleetRadar,
    Cluster { id: String },
    Agent { cluster_id: String, agent_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InitialScreen {
    Launcher,
    Monitor,
}

impl InitialScreen {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_lowercase().as_str() {
            "launcher" => Ok(Self::Launcher),
            "monitor" => Ok(Self::Monitor),
            other => Err(format!(
                "Unknown initial screen: {other}. Valid: launcher, monitor"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UiVariant {
    #[default]
    Classic,
    Disruptive,
}

impl UiVariant {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_lowercase().as_str() {
            "classic" => Ok(Self::Classic),
            "disruptive" => Ok(Self::Disruptive),
            other => Err(format!(
                "Unknown UI variant: {other}. Valid: classic, disruptive"
            )),
        }
    }
}

pub fn resolve_ui_variant(
    cli_value: Option<&str>,
    env_value: Option<&str>,
) -> Result<Option<UiVariant>, String> {
    if let Some(raw) = cli_value {
        if !raw.trim().is_empty() {
            return Ok(Some(UiVariant::parse(raw)?));
        }
    }

    if let Some(raw) = env_value {
        if !raw.trim().is_empty() {
            return Ok(Some(UiVariant::parse(raw)?));
        }
    }

    Ok(None)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub position: (f32, f32),
    pub zoom: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            position: (0.0, 0.0),
            zoom: 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TimeCursorMode {
    #[default]
    Live,
    Scrub,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeCursor {
    pub mode: TimeCursorMode,
    pub cursor_ms: i64,
}

impl Default for TimeCursor {
    fn default() -> Self {
        Self {
            mode: TimeCursorMode::Live,
            cursor_ms: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SpineMode {
    #[default]
    Intent,
    Command,
    WhisperCluster,
    WhisperAgent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpineCompletion {
    pub text: String,
    pub selection: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpineState {
    pub mode: SpineMode,
    pub input: InputState,
    pub hint: SpineHint,
    pub completion: Option<SpineCompletion>,
}

impl Default for SpineState {
    fn default() -> Self {
        Self {
            mode: SpineMode::Intent,
            input: InputState::default(),
            hint: SpineHint::default(),
            completion: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StartupOptions {
    pub initial_screen: Option<InitialScreen>,
    pub provider_override: Option<String>,
    pub ui_variant: Option<UiVariant>,
}

#[derive(Debug, Clone)]
pub enum BackendStatus {
    Disconnected,
    Connected,
    Error(String),
    Exited(BackendExit),
}

#[derive(Debug, Clone, Default)]
pub struct CommandBarState {
    pub active: bool,
    inner: InputState,
}

impl CommandBarState {
    /// Read-only access to input text.
    pub fn input(&self) -> &str {
        &self.inner.input
    }

    /// Read-only access to cursor position.
    pub fn cursor(&self) -> usize {
        self.inner.cursor
    }

    pub fn open_with(&mut self, prefill: String) {
        self.active = true;
        self.inner.input = prefill;
        self.inner.move_end();
    }

    pub fn close(&mut self) {
        self.active = false;
        self.inner.clear();
    }

    pub fn insert_char(&mut self, ch: char) {
        self.inner.insert_char(ch);
    }

    pub fn backspace(&mut self) {
        self.inner.backspace();
    }

    pub fn delete(&mut self) {
        self.inner.delete();
    }

    pub fn move_left(&mut self) {
        self.inner.move_left();
    }

    pub fn move_right(&mut self) {
        self.inner.move_right();
    }

    pub fn move_home(&mut self) {
        self.inner.move_home();
    }

    pub fn move_end(&mut self) {
        self.inner.move_end();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToastLevel {
    Info,
    Success,
    Error,
}

#[derive(Debug, Clone)]
pub struct ToastState {
    pub message: String,
    pub level: ToastLevel,
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandContext {
    pub provider_override: Option<String>,
    pub active_screen: ScreenId,
    pub ui_variant: UiVariant,
}

#[derive(Debug, Clone)]
pub struct AppState {
    pub screen_stack: Vec<ScreenId>,
    pub launcher: launcher::State,
    pub monitor: monitor::State,
    pub metrics: HashMap<String, ClusterMetrics>,
    pub last_metrics_poll_at: Option<i64>,
    pub clusters: HashMap<String, cluster::State>,
    pub agents: HashMap<AgentKey, agent::State>,
    pub last_size: Option<(u16, u16)>,
    pub tick_count: u64,
    pub now_ms: i64,
    pub should_quit: bool,
    pub backend_status: BackendStatus,
    pub last_error: Option<String>,
    pub provider_override: Option<String>,
    pub ui_variant: UiVariant,
    pub camera: Camera,
    pub time_cursor: TimeCursor,
    pub spine: SpineState,
    pub command_bar: CommandBarState,
    pub toast: Option<ToastState>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            screen_stack: vec![ScreenId::Launcher],
            launcher: launcher::State::default(),
            monitor: monitor::State::default(),
            metrics: HashMap::new(),
            last_metrics_poll_at: None,
            clusters: HashMap::new(),
            agents: HashMap::new(),
            last_size: None,
            tick_count: 0,
            now_ms: 0,
            should_quit: false,
            backend_status: BackendStatus::Disconnected,
            last_error: None,
            provider_override: None,
            ui_variant: UiVariant::Classic,
            camera: Camera::default(),
            time_cursor: TimeCursor::default(),
            spine: SpineState::default(),
            command_bar: CommandBarState::default(),
            toast: None,
        }
    }
}

impl AppState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply_startup_options(&mut self, options: StartupOptions) {
        if let Some(provider) = options.provider_override {
            self.provider_override = Some(provider);
        }

        if let Some(ui_variant) = options.ui_variant {
            self.ui_variant = ui_variant;
        }

        let initial_screen = options.initial_screen;
        if matches!(self.ui_variant, UiVariant::Disruptive) {
            let mut stack = vec![ScreenId::IntentConsole];
            if matches!(initial_screen, Some(InitialScreen::Monitor)) {
                stack.push(ScreenId::FleetRadar);
            }
            self.screen_stack = stack;
        } else if let Some(initial_screen) = initial_screen {
            self.screen_stack = vec![ScreenId::Launcher];
            match initial_screen {
                InitialScreen::Launcher => {}
                InitialScreen::Monitor => {
                    self.screen_stack.push(ScreenId::Monitor);
                }
            }
        }
    }

    pub fn active_screen(&self) -> &ScreenId {
        self.screen_stack
            .last()
            .unwrap_or(&ScreenId::Launcher)
    }

    pub fn zoom_stack_context(&self) -> ZoomStackContext {
        for screen in self.screen_stack.iter().rev() {
            match screen {
                ScreenId::Agent {
                    cluster_id,
                    agent_id,
                }
                | ScreenId::AgentMicroscope {
                    cluster_id,
                    agent_id,
                } => {
                    return ZoomStackContext::Agent {
                        cluster_id: cluster_id.clone(),
                        agent_id: agent_id.clone(),
                    };
                }
                ScreenId::Cluster { id } | ScreenId::ClusterCanvas { id } => {
                    return ZoomStackContext::Cluster { id: id.clone() };
                }
                ScreenId::Monitor | ScreenId::FleetRadar => {
                    return ZoomStackContext::FleetRadar;
                }
                ScreenId::Launcher | ScreenId::IntentConsole => {}
            }
        }
        ZoomStackContext::Root
    }

    pub fn command_context(&self) -> CommandContext {
        CommandContext {
            provider_override: self.provider_override.clone(),
            active_screen: self.active_screen().clone(),
            ui_variant: self.ui_variant,
        }
    }

    fn metrics_poll_due(&self, now_ms: i64) -> bool {
        match self.last_metrics_poll_at {
            None => true,
            Some(last) => now_ms.saturating_sub(last) >= METRICS_POLL_INTERVAL_MS,
        }
    }

    fn mark_metrics_polled(&mut self, now_ms: i64) {
        self.last_metrics_poll_at = Some(now_ms);
    }

    fn ensure_screen_state(&mut self, screen: &ScreenId) {
        match screen {
            ScreenId::Launcher
            | ScreenId::Monitor
            | ScreenId::IntentConsole
            | ScreenId::FleetRadar => {}
            ScreenId::Cluster { id } | ScreenId::ClusterCanvas { id } => {
                self.clusters
                    .entry(id.clone())
                    .or_default();
            }
            ScreenId::Agent {
                cluster_id,
                agent_id,
            }
            | ScreenId::AgentMicroscope {
                cluster_id,
                agent_id,
            } => {
                let key = AgentKey::new(cluster_id.clone(), agent_id.clone());
                self.agents
                    .entry(key)
                    .or_default();
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
    ClusterMetricsListed { metrics: Vec<ClusterMetrics> },
    ClusterSummary {
        summary: crate::protocol::ClusterSummary,
    },
    ClusterTopology {
        cluster_id: String,
        topology: crate::protocol::ClusterTopology,
    },
    ClusterTopologyError {
        cluster_id: String,
        message: String,
    },
    SubscribedClusterLogs {
        cluster_id: String,
        agent_id: Option<String>,
        subscription_id: String,
    },
    SubscribedClusterTimeline {
        cluster_id: String,
        subscription_id: String,
    },
    GuidanceToAgentResult {
        cluster_id: String,
        agent_id: String,
        result: crate::protocol::GuidanceDeliveryResult,
    },
    GuidanceToAgentError {
        cluster_id: String,
        agent_id: String,
        message: String,
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
    CommandBar(CommandBarAction),
    Spine(SpineAction),
    Command(CommandAction),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Backend(BackendRequest),
    Command(CommandRequest),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendRequest {
    ListClusters,
    ListClusterMetrics { cluster_ids: Option<Vec<String>> },
    GetClusterSummary { cluster_id: String },
    GetClusterTopology { cluster_id: String },
    SubscribeClusterLogs {
        cluster_id: String,
        agent_id: Option<String>,
    },
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
    SubmitRaw { raw: String, context: CommandContext },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandBarAction {
    Open { prefill: String },
    Close,
    InsertChar(char),
    Backspace,
    Delete,
    MoveCursorLeft,
    MoveCursorRight,
    MoveCursorHome,
    MoveCursorEnd,
    Submit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpineAction {
    SetMode(SpineMode),
    SetHint(SpineHint),
    SetCompletion(Option<SpineCompletion>),
    EnterMode { mode: SpineMode, prefill: String },
    Cancel,
    Submit,
    Complete,
    InsertChar(char),
    Backspace,
    Delete,
    MoveCursorLeft,
    MoveCursorRight,
    MoveCursorHome,
    MoveCursorEnd,
    Clear,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandAction {
    ShowToast { level: ToastLevel, message: String },
    SetProviderOverride { provider: Option<String> },
    StartClusterFromIssue {
        reference: String,
        provider_override: Option<String>,
    },
}

const TOAST_DURATION_MS: i64 = 5000;
const METRICS_POLL_INTERVAL_MS: i64 = 2000;

pub fn update(mut state: AppState, action: Action) -> (AppState, Vec<Effect>) {
    let mut effects = Vec::new();
    match action {
        Action::Tick { now_ms } => {
            state.tick_count = state.tick_count.saturating_add(1);
            state.now_ms = now_ms;
            if let Some(toast) = &state.toast {
                if toast.expires_at_ms <= state.now_ms {
                    state.toast = None;
                }
            }
            let should_poll = matches!(
                state.active_screen(),
                ScreenId::Monitor | ScreenId::FleetRadar
            )
                && state.monitor.poll_due(now_ms);
            if should_poll {
                state.monitor.mark_polled(now_ms);
                effects.push(Effect::Backend(BackendRequest::ListClusters));
            }
            let should_poll_metrics = matches!(
                state.active_screen(),
                ScreenId::Monitor
                    | ScreenId::FleetRadar
                    | ScreenId::Cluster { .. }
                    | ScreenId::ClusterCanvas { .. }
            ) && state.metrics_poll_due(now_ms);
            if should_poll_metrics {
                if let Some(request) = metrics_request_for_screen(&state) {
                    state.mark_metrics_polled(now_ms);
                    effects.push(Effect::Backend(request));
                }
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
        Action::CommandBar(command_action) => {
            handle_command_bar_action(&mut state, command_action, &mut effects);
        }
        Action::Spine(spine_action) => {
            handle_spine_action(&mut state, spine_action, &mut effects);
        }
        Action::Command(command_action) => {
            handle_command_action(&mut state, command_action, &mut effects);
        }
    }

    (state, effects)
}

fn apply_navigation(state: &mut AppState, nav: NavigationAction, effects: &mut Vec<Effect>) {
    match nav {
        NavigationAction::Push(screen) => {
            cleanup_active_screen(state, effects);
            seed_agent_role_for_navigation(state, &screen);
            state.ensure_screen_state(&screen);
            if matches!(screen, ScreenId::Monitor | ScreenId::FleetRadar) {
                state.monitor.mark_polled(state.now_ms);
            }
            state.screen_stack.push(screen.clone());
            queue_navigation_effects(&screen, effects);
        }
        NavigationAction::Pop => {
            if state.screen_stack.len() > 1 {
                cleanup_active_screen(state, effects);
                state.screen_stack.pop();
                if let Some(active) = state.screen_stack.last() {
                    if matches!(active, ScreenId::Monitor | ScreenId::FleetRadar) {
                        state.monitor.mark_polled(state.now_ms);
                    }
                    queue_navigation_effects(active, effects);
                }
            }
        }
        NavigationAction::ReplaceTop(screen) => {
            cleanup_active_screen(state, effects);
            seed_agent_role_for_navigation(state, &screen);
            state.ensure_screen_state(&screen);
            if matches!(screen, ScreenId::Monitor | ScreenId::FleetRadar) {
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

    refresh_spine_hint(state);
}

fn seed_agent_role_for_navigation(state: &mut AppState, screen: &ScreenId) {
    let (cluster_id, agent_id) = match screen {
        ScreenId::Agent {
            cluster_id,
            agent_id,
        }
        | ScreenId::AgentMicroscope {
            cluster_id,
            agent_id,
        } => (cluster_id, agent_id),
        _ => return,
    };

    let role = state
        .clusters
        .get(cluster_id)
        .and_then(|cluster_state| {
            cluster_state
                .agents
                .iter()
                .find(|agent| agent.id == *agent_id)
                .and_then(|agent| agent.role.clone())
        });
    seed_agent_role(state, cluster_id, agent_id, role);
}

fn queue_navigation_effects(screen: &ScreenId, effects: &mut Vec<Effect>) {
    match screen {
        ScreenId::Monitor | ScreenId::FleetRadar => {
            effects.push(Effect::Backend(BackendRequest::ListClusters));
        }
        ScreenId::Cluster { id } | ScreenId::ClusterCanvas { id } => {
            effects.push(Effect::Backend(BackendRequest::GetClusterSummary {
                cluster_id: id.clone(),
            }));
            effects.push(Effect::Backend(BackendRequest::GetClusterTopology {
                cluster_id: id.clone(),
            }));
            effects.push(Effect::Backend(BackendRequest::SubscribeClusterLogs {
                cluster_id: id.clone(),
                agent_id: None,
            }));
            effects.push(Effect::Backend(BackendRequest::SubscribeClusterTimeline {
                cluster_id: id.clone(),
            }));
        }
        ScreenId::Agent {
            cluster_id,
            agent_id,
        }
        | ScreenId::AgentMicroscope {
            cluster_id,
            agent_id,
        } => {
            effects.push(Effect::Backend(BackendRequest::SubscribeClusterLogs {
                cluster_id: cluster_id.clone(),
                agent_id: Some(agent_id.clone()),
            }));
        }
        ScreenId::Launcher | ScreenId::IntentConsole => {}
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
        } => handle_agent_action(state, cluster_id, agent_id, action, effects),
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
                    context: state.command_context(),
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
    match action {
        cluster::Action::CycleFocus(direction) => {
            let entry = state
                .clusters
                .entry(id)
                .or_default();
            entry.cycle_focus(direction);
        }
        cluster::Action::MoveFocused(delta) => {
            let entry = state
                .clusters
                .entry(id)
                .or_default();
            entry.move_focused(delta);
        }
        cluster::Action::ActivateFocused => {
            let (agent_id, role) = {
                let entry = state
                    .clusters
                    .entry(id.clone())
                    .or_default();
                let agent_id = entry.activate_focused();
                let role = agent_id
                    .as_ref()
                    .and_then(|selected| {
                        entry
                            .agents
                            .iter()
                            .find(|agent| agent.id == *selected)
                            .and_then(|agent| agent.role.clone())
                    });
                (agent_id, role)
            };
            if let Some(agent_id) = agent_id {
                seed_agent_role(state, &id, &agent_id, role);
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
        cluster::Action::OpenAgent(agent_id) => {
            let role = {
                let entry = state
                    .clusters
                    .entry(id.clone())
                    .or_default();
                entry
                    .agents
                    .iter()
                    .find(|agent| agent.id == agent_id)
                    .and_then(|agent| agent.role.clone())
            };
            seed_agent_role(state, &id, &agent_id, role);
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

fn seed_agent_role(
    state: &mut AppState,
    cluster_id: &str,
    agent_id: &str,
    role: Option<String>,
) {
    if let Some(role) = role {
        let key = AgentKey::new(cluster_id.to_string(), agent_id.to_string());
        let entry = state
            .agents
            .entry(key)
            .or_default();
        if entry.role.is_none() {
            entry.role = Some(role);
        }
    }
}

fn handle_agent_action(
    state: &mut AppState,
    cluster_id: String,
    agent_id: String,
    action: agent::Action,
    effects: &mut Vec<Effect>,
) {
    let key = AgentKey::new(cluster_id.clone(), agent_id.clone());
    let entry = state
        .agents
        .entry(key)
        .or_default();
    match action {
        agent::Action::SubmitGuidance => {
            let trimmed = entry.guidance_input.input.trim();
            if trimmed.is_empty() {
                entry.apply_guidance_error("Enter guidance text.".to_string());
                return;
            }
            entry.guidance_pending = true;
            entry.last_guidance_error = None;
            effects.push(Effect::Backend(BackendRequest::SendGuidanceToAgent {
                cluster_id,
                agent_id,
                message: trimmed.to_string(),
            }));
        }
        agent::Action::InsertChar(ch) => {
            entry.guidance_input.insert_char(ch);
        }
        agent::Action::Backspace => {
            entry.guidance_input.backspace();
        }
        agent::Action::Delete => {
            entry.guidance_input.delete();
        }
        agent::Action::MoveCursorLeft => {
            entry.guidance_input.move_left();
        }
        agent::Action::MoveCursorRight => {
            entry.guidance_input.move_right();
        }
        agent::Action::MoveCursorHome => {
            entry.guidance_input.move_home();
        }
        agent::Action::MoveCursorEnd => {
            entry.guidance_input.move_end();
        }
        agent::Action::ScrollLogs(delta) => {
            entry.move_log_scroll(delta);
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
        BackendAction::ClusterMetricsListed { metrics } => {
            handle_cluster_metrics_listed(state, metrics)
        }
        BackendAction::ClusterSummary { summary } => handle_cluster_summary(state, summary),
        BackendAction::ClusterTopology {
            cluster_id,
            topology,
        } => handle_cluster_topology(state, cluster_id, topology),
        BackendAction::ClusterTopologyError { cluster_id, message } => {
            handle_cluster_topology_error(state, cluster_id, message)
        }
        BackendAction::SubscribedClusterLogs {
            cluster_id,
            agent_id,
            subscription_id,
        } => handle_log_subscription(state, cluster_id, agent_id, subscription_id),
        BackendAction::SubscribedClusterTimeline {
            cluster_id,
            subscription_id,
        } => handle_cluster_timeline_subscription(state, cluster_id, subscription_id),
        BackendAction::GuidanceToAgentResult {
            cluster_id,
            agent_id,
            result,
        } => handle_guidance_result(state, cluster_id, agent_id, result),
        BackendAction::GuidanceToAgentError {
            cluster_id,
            agent_id,
            message,
        } => handle_guidance_error(state, cluster_id, agent_id, message),
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
            if let Some(entry) = state
                .agents
                .values_mut()
                .find(|agent| agent.log_subscription.as_deref() == Some(params.subscription_id.as_str()))
            {
                if entry.role.is_none() {
                    let role = params.lines.iter().find_map(|line| line.role.clone());
                    if let Some(role) = role {
                        entry.role = Some(role);
                    }
                }
                entry.push_log_lines(params.lines, params.dropped_count);
                return;
            }

            if let Some(entry) = state.clusters.get_mut(&params.cluster_id) {
                if entry.log_subscription.as_deref() == Some(params.subscription_id.as_str()) {
                    entry.push_log_lines(params.lines, params.dropped_count);
                }
            }
        }
        BackendNotification::ClusterTimelineEvents(params) => {
            let entry = state
                .clusters
                .entry(params.cluster_id)
                .or_default();
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
    let ids: HashSet<String> = state
        .monitor
        .clusters
        .iter()
        .map(|cluster| cluster.id.clone())
        .collect();
    state.metrics.retain(|id, _| ids.contains(id));
}

fn handle_cluster_metrics_listed(state: &mut AppState, metrics: Vec<ClusterMetrics>) {
    for metric in metrics {
        state.metrics.insert(metric.id.clone(), metric);
    }
}

fn handle_cluster_summary(state: &mut AppState, summary: crate::protocol::ClusterSummary) {
    let entry = state
        .clusters
        .entry(summary.id.clone())
        .or_default();
    entry.summary = Some(summary);
}

fn handle_cluster_topology(
    state: &mut AppState,
    cluster_id: String,
    topology: crate::protocol::ClusterTopology,
) {
    let entry = state
        .clusters
        .entry(cluster_id)
        .or_default();
    entry.topology = Some(topology);
    entry.topology_error = None;
}

fn handle_cluster_topology_error(state: &mut AppState, cluster_id: String, message: String) {
    let entry = state
        .clusters
        .entry(cluster_id)
        .or_default();
    entry.topology = None;
    entry.topology_error = Some(message);
}

fn handle_log_subscription(
    state: &mut AppState,
    cluster_id: String,
    agent_id: Option<String>,
    subscription_id: String,
) {
    match agent_id {
        Some(agent_id) => {
            let key = AgentKey::new(cluster_id, agent_id);
            let entry = state
                .agents
                .entry(key)
                .or_default();
            entry.log_subscription = Some(subscription_id);
        }
        None => {
            let entry = state
                .clusters
                .entry(cluster_id)
                .or_default();
            entry.log_subscription = Some(subscription_id);
        }
    }
}

fn handle_cluster_timeline_subscription(
    state: &mut AppState,
    cluster_id: String,
    subscription_id: String,
) {
    let entry = state
        .clusters
        .entry(cluster_id)
        .or_default();
    entry.timeline_subscription = Some(subscription_id);
}

fn handle_guidance_result(
    state: &mut AppState,
    cluster_id: String,
    agent_id: String,
    result: crate::protocol::GuidanceDeliveryResult,
) {
    let key = AgentKey::new(cluster_id, agent_id);
    let entry = state
        .agents
        .entry(key)
        .or_default();
    entry.apply_guidance_result(result);
}

fn handle_guidance_error(
    state: &mut AppState,
    cluster_id: String,
    agent_id: String,
    message: String,
) {
    let key = AgentKey::new(cluster_id, agent_id);
    let entry = state
        .agents
        .entry(key)
        .or_default();
    entry.apply_guidance_error(message.clone());
    state.last_error = Some(message);
}

fn handle_start_cluster_result(
    state: &mut AppState,
    cluster_id: String,
    effects: &mut Vec<Effect>,
) {
    state.launcher.clear();
    let screen = if matches!(state.ui_variant, UiVariant::Disruptive) {
        ScreenId::ClusterCanvas { id: cluster_id }
    } else {
        ScreenId::Cluster { id: cluster_id }
    };
    apply_navigation(state, NavigationAction::Push(screen), effects);
}

fn handle_backend_error(state: &mut AppState, message: String) {
    state.last_error = Some(message);
}

fn handle_command_bar_action(
    state: &mut AppState,
    action: CommandBarAction,
    effects: &mut Vec<Effect>,
) {
    match action {
        CommandBarAction::Open { prefill } => {
            state.command_bar.open_with(prefill);
        }
        CommandBarAction::Close => {
            state.command_bar.close();
        }
        CommandBarAction::InsertChar(ch) => {
            if state.command_bar.active {
                state.command_bar.insert_char(ch);
            }
        }
        CommandBarAction::Backspace => {
            if state.command_bar.active {
                state.command_bar.backspace();
            }
        }
        CommandBarAction::Delete => {
            if state.command_bar.active {
                state.command_bar.delete();
            }
        }
        CommandBarAction::MoveCursorLeft => {
            if state.command_bar.active {
                state.command_bar.move_left();
            }
        }
        CommandBarAction::MoveCursorRight => {
            if state.command_bar.active {
                state.command_bar.move_right();
            }
        }
        CommandBarAction::MoveCursorHome => {
            if state.command_bar.active {
                state.command_bar.move_home();
            }
        }
        CommandBarAction::MoveCursorEnd => {
            if state.command_bar.active {
                state.command_bar.move_end();
            }
        }
        CommandBarAction::Submit => {
            let raw = state.command_bar.input().to_string();
            let context = state.command_context();
            state.command_bar.close();
            effects.push(Effect::Command(CommandRequest::SubmitRaw { raw, context }));
        }
    }
}

fn set_spine_input(state: &mut AppState, value: String) {
    state.spine.input.input = value;
    state.spine.input.cursor = state.spine.input.input.chars().count();
}

fn reset_spine_state(state: &mut AppState) {
    state.spine.mode = SpineMode::Intent;
    state.spine.input.clear();
    state.spine.completion = None;
    state.spine.hint = SpineHint::default();
}

fn set_disruptive_spine_toast(state: &mut AppState, level: ToastLevel, message: String) {
    if !matches!(state.ui_variant, UiVariant::Disruptive) {
        return;
    }
    state.toast = Some(ToastState {
        message,
        level,
        expires_at_ms: state.now_ms.saturating_add(TOAST_DURATION_MS),
    });
}

fn refresh_spine_hint(state: &mut AppState) {
    state.spine.hint = compute_spine_hint(state);
}

fn detect_issue_reference(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.chars().all(|ch| ch.is_ascii_digit()) {
        return Some(trimmed.to_string());
    }
    if let Some(reference) = parse_owner_repo_issue(trimmed) {
        return Some(reference);
    }
    parse_github_issue_url(trimmed)
}

fn parse_owner_repo_issue(input: &str) -> Option<String> {
    let mut parts = input.split('#');
    let repo_ref = parts.next()?;
    let number = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    if number.is_empty() || !number.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    let mut repo_parts = repo_ref.split('/');
    let owner = repo_parts.next()?;
    let repo = repo_parts.next()?;
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    if repo_parts.next().is_some() {
        return None;
    }
    Some(format!("{owner}/{repo}#{number}"))
}

fn parse_github_issue_url(input: &str) -> Option<String> {
    let trimmed = input.trim();
    let without_scheme = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .unwrap_or(trimmed);
    let without_host = without_scheme.strip_prefix("github.com/")?;
    let mut parts = without_host.split('/');
    let owner = parts.next()?;
    let repo = parts.next()?;
    let issues = parts.next()?;
    if owner.is_empty() || repo.is_empty() || issues != "issues" {
        return None;
    }
    let number_segment = parts.next()?;
    let number = number_segment.split(['?', '#']).next().unwrap_or("");
    if number.is_empty() || !number.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    Some(format!("{owner}/{repo}#{number}"))
}

fn resolve_spine_cluster_target(state: &AppState) -> Option<String> {
    match state.zoom_stack_context() {
        ZoomStackContext::Agent { cluster_id, .. } => Some(cluster_id),
        ZoomStackContext::Cluster { id } => Some(id),
        ZoomStackContext::FleetRadar => state.monitor.selected_cluster_id(),
        ZoomStackContext::Root => None,
    }
}

fn resolve_spine_agent_target(state: &AppState) -> Option<(String, String)> {
    match state.zoom_stack_context() {
        ZoomStackContext::Agent {
            cluster_id,
            agent_id,
        } => Some((cluster_id, agent_id)),
        ZoomStackContext::Cluster { id } => {
            let cluster_state = state.clusters.get(&id)?;
            let agent = cluster_state.agents.get(cluster_state.selected_agent)?;
            Some((id, agent.id.clone()))
        }
        ZoomStackContext::FleetRadar | ZoomStackContext::Root => None,
    }
}

fn handle_spine_action(state: &mut AppState, action: SpineAction, effects: &mut Vec<Effect>) {
    let mut should_refresh_hint = false;
    match action {
        SpineAction::SetMode(mode) => {
            state.spine.mode = mode;
            should_refresh_hint = true;
        }
        SpineAction::SetHint(hint) => {
            state.spine.hint = hint;
        }
        SpineAction::SetCompletion(completion) => {
            state.spine.completion = completion;
        }
        SpineAction::EnterMode { mode, prefill } => {
            state.spine.mode = mode;
            set_spine_input(state, prefill);
            state.spine.completion = None;
            should_refresh_hint = true;
        }
        SpineAction::Cancel => {
            reset_spine_state(state);
            should_refresh_hint = true;
        }
        SpineAction::Submit => {
            let mode = state.spine.mode;
            let raw_input = state.spine.input.input.clone();
            let trimmed = raw_input.trim();
            let mut toast_message: Option<String> = None;
            match mode {
                SpineMode::Command => {
                    let raw = if raw_input.starts_with('/') {
                        raw_input
                    } else {
                        format!("/{}", raw_input)
                    };
                    let context = state.command_context();
                    effects.push(Effect::Command(CommandRequest::SubmitRaw { raw, context }));
                }
                SpineMode::Intent => {
                    if !trimmed.is_empty() {
                        if let Some(reference) = detect_issue_reference(trimmed) {
                            effects.push(Effect::Backend(BackendRequest::StartClusterFromIssue {
                                reference: reference.clone(),
                                provider_override: state.provider_override.clone(),
                            }));
                            toast_message =
                                Some(format!("Starting cluster from issue {reference}..."));
                        } else {
                            effects.push(Effect::Backend(BackendRequest::StartClusterFromText {
                                text: trimmed.to_string(),
                                provider_override: state.provider_override.clone(),
                            }));
                            toast_message = Some("Starting cluster...".to_string());
                        }
                    }
                }
                SpineMode::WhisperCluster => {
                    if !trimmed.is_empty() {
                        if let Some(cluster_id) = resolve_spine_cluster_target(state) {
                            effects.push(Effect::Backend(BackendRequest::SendGuidanceToCluster {
                                cluster_id,
                                message: trimmed.to_string(),
                            }));
                            toast_message = Some("Whisper sent to cluster.".to_string());
                        }
                    }
                }
                SpineMode::WhisperAgent => {
                    if !trimmed.is_empty() {
                        if let Some((cluster_id, agent_id)) = resolve_spine_agent_target(state) {
                            effects.push(Effect::Backend(BackendRequest::SendGuidanceToAgent {
                                cluster_id,
                                agent_id,
                                message: trimmed.to_string(),
                            }));
                            toast_message = Some("Whisper sent to agent.".to_string());
                        }
                    }
                }
            }
            if let Some(message) = toast_message {
                set_disruptive_spine_toast(state, ToastLevel::Success, message);
            }
            reset_spine_state(state);
            should_refresh_hint = true;
        }
        SpineAction::Complete => {
            if let Some(completion) = state.spine.completion.take() {
                if !completion.text.is_empty() {
                    state.spine.input.input.push_str(completion.text.as_str());
                    state.spine.input.cursor = state.spine.input.input.chars().count();
                }
            }
            should_refresh_hint = true;
        }
        SpineAction::InsertChar(ch) => {
            state.spine.input.insert_char(ch);
            should_refresh_hint = true;
        }
        SpineAction::Backspace => {
            state.spine.input.backspace();
            should_refresh_hint = true;
        }
        SpineAction::Delete => {
            state.spine.input.delete();
            should_refresh_hint = true;
        }
        SpineAction::MoveCursorLeft => {
            state.spine.input.move_left();
        }
        SpineAction::MoveCursorRight => {
            state.spine.input.move_right();
        }
        SpineAction::MoveCursorHome => {
            state.spine.input.move_home();
        }
        SpineAction::MoveCursorEnd => {
            state.spine.input.move_end();
        }
        SpineAction::Clear => {
            state.spine.input.clear();
            state.spine.completion = None;
            should_refresh_hint = true;
        }
    }

    if should_refresh_hint {
        refresh_spine_hint(state);
    }
}

fn handle_command_action(state: &mut AppState, action: CommandAction, effects: &mut Vec<Effect>) {
    match action {
        CommandAction::ShowToast { level, message } => {
            state.toast = Some(ToastState {
                message,
                level,
                expires_at_ms: state.now_ms.saturating_add(TOAST_DURATION_MS),
            });
        }
        CommandAction::SetProviderOverride { provider } => {
            state.provider_override = provider;
            refresh_spine_hint(state);
        }
        CommandAction::StartClusterFromIssue {
            reference,
            provider_override,
        } => {
            effects.push(Effect::Backend(BackendRequest::StartClusterFromIssue {
                reference,
                provider_override,
            }));
        }
    }
}

fn cleanup_active_screen(state: &mut AppState, effects: &mut Vec<Effect>) {
    let active = state.screen_stack.last().cloned();
    match active {
        Some(ScreenId::Cluster { id }) | Some(ScreenId::ClusterCanvas { id }) => {
            cleanup_cluster_subscriptions(state, &id, effects)
        }
        Some(ScreenId::Agent {
            cluster_id,
            agent_id,
        })
        | Some(ScreenId::AgentMicroscope {
            cluster_id,
            agent_id,
        }) => cleanup_agent_subscriptions(state, &cluster_id, &agent_id, effects),
        _ => {}
    }
}

fn cleanup_cluster_subscriptions(state: &mut AppState, id: &str, effects: &mut Vec<Effect>) {
    let Some(entry) = state.clusters.get_mut(id) else {
        return;
    };
    if let Some(subscription_id) = entry.log_subscription.take() {
        effects.push(Effect::Backend(BackendRequest::Unsubscribe { subscription_id }));
    }
    if let Some(subscription_id) = entry.timeline_subscription.take() {
        effects.push(Effect::Backend(BackendRequest::Unsubscribe { subscription_id }));
    }
}

fn cleanup_agent_subscriptions(
    state: &mut AppState,
    cluster_id: &str,
    agent_id: &str,
    effects: &mut Vec<Effect>,
) {
    let key = AgentKey::new(cluster_id.to_string(), agent_id.to_string());
    let Some(entry) = state.agents.get_mut(&key) else {
        return;
    };
    if let Some(subscription_id) = entry.log_subscription.take() {
        effects.push(Effect::Backend(BackendRequest::Unsubscribe { subscription_id }));
    }
}

fn metrics_request_for_screen(state: &AppState) -> Option<BackendRequest> {
    match state.active_screen() {
        ScreenId::Monitor | ScreenId::FleetRadar => {
            let ids: Vec<String> = state
                .monitor
                .clusters
                .iter()
                .map(|cluster| cluster.id.clone())
                .collect();
            if ids.is_empty() {
                None
            } else {
                Some(BackendRequest::ListClusterMetrics {
                    cluster_ids: Some(ids),
                })
            }
        }
        ScreenId::Cluster { id } | ScreenId::ClusterCanvas { id } => {
            Some(BackendRequest::ListClusterMetrics {
            cluster_ids: Some(vec![id.clone()]),
        })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands;

    fn apply_actions(mut state: AppState, actions: Vec<Action>) -> (AppState, Vec<Effect>) {
        let mut effects = Vec::new();
        for action in actions {
            let (next_state, next_effects) = update(state, action);
            state = next_state;
            effects.extend(next_effects);
        }
        (state, effects)
    }

    #[test]
    fn provider_override_applies_to_issue_start() {
        let state = AppState::default();
        let actions = commands::dispatch(CommandRequest::SubmitRaw {
            raw: "/provider codex".to_string(),
            context: state.command_context(),
        })
        .expect("dispatch provider");
        let (state, _) = apply_actions(state, actions);
        assert_eq!(state.provider_override, Some("codex".to_string()));

        let actions = commands::dispatch(CommandRequest::SubmitRaw {
            raw: "/issue org/repo#123".to_string(),
            context: state.command_context(),
        })
        .expect("dispatch issue");
        let (_state, effects) = apply_actions(state, actions);
        let mut found = false;
        for effect in effects {
            if let Effect::Backend(BackendRequest::StartClusterFromIssue {
                reference,
                provider_override,
            }) = effect
            {
                found = true;
                assert_eq!(reference, "org/repo#123");
                assert_eq!(provider_override, Some("codex".to_string()));
            }
        }
        assert!(found, "expected StartClusterFromIssue effect");
    }

    #[test]
    fn spine_state_editing() {
        let mut state = AppState::default();

        let (next, _) = update(state, Action::Spine(SpineAction::InsertChar('a')));
        state = next;
        assert_eq!(state.spine.input.input, "a");
        assert_eq!(state.spine.input.cursor, 1);

        let (next, _) = update(state, Action::Spine(SpineAction::InsertChar('b')));
        state = next;
        assert_eq!(state.spine.input.input, "ab");
        assert_eq!(state.spine.input.cursor, 2);

        let (next, _) = update(state, Action::Spine(SpineAction::InsertChar('c')));
        state = next;
        assert_eq!(state.spine.input.input, "abc");
        assert_eq!(state.spine.input.cursor, 3);

        let (next, _) = update(state, Action::Spine(SpineAction::MoveCursorLeft));
        state = next;
        assert_eq!(state.spine.input.cursor, 2);

        let (next, _) = update(state, Action::Spine(SpineAction::Backspace));
        state = next;
        assert_eq!(state.spine.input.input, "ac");
        assert_eq!(state.spine.input.cursor, 1);

        let (next, _) = update(state, Action::Spine(SpineAction::Delete));
        state = next;
        assert_eq!(state.spine.input.input, "a");
        assert_eq!(state.spine.input.cursor, 1);

        let (next, _) = update(state, Action::Spine(SpineAction::MoveCursorHome));
        state = next;
        assert_eq!(state.spine.input.cursor, 0);

        let (next, _) = update(state, Action::Spine(SpineAction::InsertChar('z')));
        state = next;
        assert_eq!(state.spine.input.input, "za");
        assert_eq!(state.spine.input.cursor, 1);

        let (next, _) = update(state, Action::Spine(SpineAction::MoveCursorEnd));
        state = next;
        assert_eq!(state.spine.input.cursor, 2);

        let (next, _) = update(state, Action::Spine(SpineAction::MoveCursorRight));
        state = next;
        assert_eq!(state.spine.input.cursor, 2);

        let (next, _) = update(state, Action::Spine(SpineAction::Clear));
        state = next;
        assert_eq!(state.spine.input.input, "");
        assert_eq!(state.spine.input.cursor, 0);
    }

    #[test]
    fn spine_cancel_resets_mode_and_input() {
        let mut state = AppState::default();
        state.spine.mode = SpineMode::Command;
        state.spine.input.input = "help".to_string();
        state.spine.input.cursor = 4;
        state.spine.completion = Some(SpineCompletion {
            text: "er".to_string(),
            selection: 0,
        });

        let (state, effects) = update(state, Action::Spine(SpineAction::Cancel));
        assert!(effects.is_empty());
        assert_eq!(state.spine.mode, SpineMode::Intent);
        assert_eq!(state.spine.input.input, "");
        assert_eq!(state.spine.input.cursor, 0);
        assert!(state.spine.completion.is_none());
    }

    #[test]
    fn spine_submit_command_emits_command_effect() {
        let mut state = AppState::default();
        state.spine.mode = SpineMode::Command;
        state.spine.input.input = "help ".to_string();
        state.spine.input.cursor = 5;

        let (state, effects) = update(state, Action::Spine(SpineAction::Submit));
        assert!(effects.iter().any(|effect| {
            matches!(
                effect,
                Effect::Command(CommandRequest::SubmitRaw { raw, .. }) if raw == "/help "
            )
        }));
        assert_eq!(state.spine.mode, SpineMode::Intent);
        assert_eq!(state.spine.input.input, "");
        assert!(state.spine.completion.is_none());
    }

    #[test]
    fn spine_submit_intent_starts_cluster() {
        let mut state = AppState::default();
        state.spine.mode = SpineMode::Intent;
        state.spine.input.input = "launch".to_string();
        state.spine.input.cursor = 6;

        let (state, effects) = update(state, Action::Spine(SpineAction::Submit));
        assert!(effects.contains(&Effect::Backend(
            BackendRequest::StartClusterFromText {
                text: "launch".to_string(),
                provider_override: None,
            }
        )));
        assert_eq!(state.spine.mode, SpineMode::Intent);
        assert_eq!(state.spine.input.input, "");
    }

    #[test]
    fn spine_submit_whisper_cluster_sends_guidance() {
        let mut state = AppState::default();
        state.screen_stack = vec![ScreenId::Cluster {
            id: "cluster-1".to_string(),
        }];
        state.spine.mode = SpineMode::WhisperCluster;
        state.spine.input.input = "ping".to_string();
        state.spine.input.cursor = 4;

        let (state, effects) = update(state, Action::Spine(SpineAction::Submit));
        assert!(effects.contains(&Effect::Backend(
            BackendRequest::SendGuidanceToCluster {
                cluster_id: "cluster-1".to_string(),
                message: "ping".to_string(),
            }
        )));
        assert_eq!(state.spine.mode, SpineMode::Intent);
        assert_eq!(state.spine.input.input, "");
    }

    #[test]
    fn spine_submit_whisper_agent_sends_guidance() {
        let mut state = AppState::default();
        state.screen_stack = vec![ScreenId::Agent {
            cluster_id: "cluster-1".to_string(),
            agent_id: "agent-1".to_string(),
        }];
        state.spine.mode = SpineMode::WhisperAgent;
        state.spine.input.input = "ping".to_string();
        state.spine.input.cursor = 4;

        let (state, effects) = update(state, Action::Spine(SpineAction::Submit));
        assert!(effects.contains(&Effect::Backend(
            BackendRequest::SendGuidanceToAgent {
                cluster_id: "cluster-1".to_string(),
                agent_id: "agent-1".to_string(),
                message: "ping".to_string(),
            }
        )));
        assert_eq!(state.spine.mode, SpineMode::Intent);
        assert_eq!(state.spine.input.input, "");
    }
}
