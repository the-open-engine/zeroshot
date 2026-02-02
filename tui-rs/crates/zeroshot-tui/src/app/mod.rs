use std::collections::{HashMap, HashSet};

use crate::backend::{BackendExit, BackendNotification};
use crate::protocol::ClusterMetrics;
use crate::screens::{agent, cluster, cluster_canvas, launcher, monitor, radar};
use crate::ui::shared::InputState;

pub mod agent_microscope;
pub mod animation;
mod spine_completion;
mod spine_hint;
use animation::{clamp_tick_delta, step_spring_f32, AnimClock};
use spine_completion::{build_spine_completion, select_spine_completion};
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
    Cluster {
        id: String,
    },
    ClusterCanvas {
        id: String,
    },
    Agent {
        cluster_id: String,
        agent_id: String,
    },
    AgentMicroscope {
        cluster_id: String,
        agent_id: String,
    },
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
    Cluster {
        id: String,
    },
    Agent {
        cluster_id: String,
        agent_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TemporalFocus {
    #[default]
    None,
    Cluster {
        id: String,
    },
    Agent {
        cluster_id: String,
        agent_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FocusTarget {
    Cluster {
        id: String,
    },
    Agent {
        cluster_id: String,
        agent_id: String,
    },
}

impl FocusTarget {
    fn label(&self) -> String {
        match self {
            FocusTarget::Cluster { id } => format!("cluster {id}"),
            FocusTarget::Agent {
                cluster_id,
                agent_id,
            } => format!("agent {agent_id} @ {cluster_id}"),
        }
    }
}

impl TemporalFocus {
    pub fn is_active(&self) -> bool {
        !matches!(self, TemporalFocus::None)
    }
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

const DEFAULT_TIME_WINDOW_MS: i64 = 60_000;
pub const TIME_SCRUB_STEP_MS: i64 = 1000;
pub const TIME_SCRUB_STEP_LARGE_MS: i64 = 5000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeCursor {
    pub mode: TimeCursorMode,
    pub t_ms: i64,
    pub window_ms: i64,
}

impl Default for TimeCursor {
    fn default() -> Self {
        Self {
            mode: TimeCursorMode::Live,
            t_ms: 0,
            window_ms: DEFAULT_TIME_WINDOW_MS,
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
    pub candidates: Vec<String>,
    pub selected: usize,
    pub ghost: String,
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
    pub fleet_radar: radar::FleetRadarState,
    pub metrics: HashMap<String, ClusterMetrics>,
    pub last_metrics_poll_at: Option<i64>,
    pub clusters: HashMap<String, cluster::State>,
    pub cluster_canvases: HashMap<String, cluster_canvas::State>,
    pub agents: HashMap<AgentKey, agent::State>,
    pub agent_microscopes: HashMap<AgentKey, agent_microscope::State>,
    pub last_size: Option<(u16, u16)>,
    pub tick_count: u64,
    pub now_ms: i64,
    pub anim_clock: AnimClock,
    pub last_tick_ms: Option<i64>,
    pub should_quit: bool,
    pub backend_status: BackendStatus,
    pub last_error: Option<String>,
    pub provider_override: Option<String>,
    pub ui_variant: UiVariant,
    pub camera: Camera,
    pub camera_target: (f32, f32),
    pub camera_velocity: (f32, f32),
    pub time_cursor: TimeCursor,
    pub temporal_focus: TemporalFocus,
    pub pinned_target: Option<FocusTarget>,
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
            fleet_radar: radar::FleetRadarState::default(),
            metrics: HashMap::new(),
            last_metrics_poll_at: None,
            clusters: HashMap::new(),
            cluster_canvases: HashMap::new(),
            agents: HashMap::new(),
            agent_microscopes: HashMap::new(),
            last_size: None,
            tick_count: 0,
            now_ms: 0,
            anim_clock: AnimClock::default(),
            last_tick_ms: None,
            should_quit: false,
            backend_status: BackendStatus::Disconnected,
            last_error: None,
            provider_override: None,
            ui_variant: UiVariant::Classic,
            camera: Camera::default(),
            camera_target: (0.0, 0.0),
            camera_velocity: (0.0, 0.0),
            time_cursor: TimeCursor::default(),
            temporal_focus: TemporalFocus::default(),
            pinned_target: None,
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
            stack.push(ScreenId::FleetRadar);
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
        self.screen_stack.last().unwrap_or(&ScreenId::Launcher)
    }

    pub fn temporal_focus_scope(&self) -> Option<TemporalFocus> {
        match self.active_screen() {
            ScreenId::ClusterCanvas { id } => Some(TemporalFocus::Cluster { id: id.clone() }),
            ScreenId::AgentMicroscope {
                cluster_id,
                agent_id,
            } => Some(TemporalFocus::Agent {
                cluster_id: cluster_id.clone(),
                agent_id: agent_id.clone(),
            }),
            _ => None,
        }
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
            ScreenId::Cluster { id } => {
                self.clusters.entry(id.clone()).or_default();
            }
            ScreenId::ClusterCanvas { id } => {
                self.clusters.entry(id.clone()).or_default();
                self.cluster_canvases.entry(id.clone()).or_default();
            }
            ScreenId::Agent {
                cluster_id,
                agent_id,
            } => {
                let key = AgentKey::new(cluster_id.clone(), agent_id.clone());
                self.agents.entry(key).or_default();
            }
            ScreenId::AgentMicroscope {
                cluster_id,
                agent_id,
            } => {
                let key = AgentKey::new(cluster_id.clone(), agent_id.clone());
                self.agents.entry(key.clone()).or_default();
                self.agent_microscopes.entry(key).or_default();
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
    FleetRadar(radar::Action),
    Cluster {
        id: String,
        action: cluster::Action,
    },
    ClusterCanvas {
        id: String,
        action: cluster_canvas::Action,
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
    ClusterMetricsListed {
        metrics: Vec<ClusterMetrics>,
    },
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
    TimeCursor(TimeCursorAction),
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
    ListClusterMetrics {
        cluster_ids: Option<Vec<String>>,
    },
    GetClusterSummary {
        cluster_id: String,
    },
    GetClusterTopology {
        cluster_id: String,
    },
    SubscribeClusterLogs {
        cluster_id: String,
        agent_id: Option<String>,
    },
    SubscribeClusterTimeline {
        cluster_id: String,
    },
    StartClusterFromText {
        text: String,
        provider_override: Option<String>,
    },
    StartClusterFromIssue {
        reference: String,
        provider_override: Option<String>,
    },
    SendGuidanceToCluster {
        cluster_id: String,
        message: String,
    },
    SendGuidanceToAgent {
        cluster_id: String,
        agent_id: String,
        message: String,
    },
    Unsubscribe {
        subscription_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandRequest {
    SubmitRaw {
        raw: String,
        context: CommandContext,
    },
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
    AcceptCompletion,
    CycleCompletion,
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
pub enum TimeCursorAction {
    Step { delta_ms: i64 },
    JumpToLive,
    ToggleFollow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandAction {
    ShowToast {
        level: ToastLevel,
        message: String,
    },
    SetProviderOverride {
        provider: Option<String>,
    },
    StartClusterFromIssue {
        reference: String,
        provider_override: Option<String>,
    },
    SendGuidance {
        message: String,
        prefix: Option<String>,
    },
    TogglePin,
}

const TOAST_DURATION_MS: i64 = 5000;
const METRICS_POLL_INTERVAL_MS: i64 = 2000;
const CAMERA_ACCEL: f32 = 0.16;
const CAMERA_FRICTION: f32 = 0.82;
const CAMERA_SNAP_EPSILON: f32 = 0.08;

pub fn update(mut state: AppState, action: Action) -> (AppState, Vec<Effect>) {
    let mut effects = Vec::new();
    match action {
        Action::Tick { now_ms } => {
            let dt_ms = clamp_tick_delta(state.last_tick_ms, now_ms);
            state.last_tick_ms = Some(now_ms);
            state.tick_count = state.tick_count.saturating_add(1);
            state.now_ms = now_ms;
            state.anim_clock.advance(now_ms);
            if let Some(toast) = &state.toast {
                if toast.expires_at_ms <= state.now_ms {
                    state.toast = None;
                }
            }
            state.fleet_radar.tick_orb_smoothing(state.now_ms, dt_ms);
            update_radar_camera_smoothing(&mut state, dt_ms);
            for canvas_state in state.cluster_canvases.values_mut() {
                canvas_state.tick_camera(dt_ms);
            }
            let should_poll = if matches!(state.ui_variant, UiVariant::Disruptive) {
                state.fleet_radar.poll_due(now_ms)
            } else {
                match state.active_screen() {
                    ScreenId::Monitor => state.monitor.poll_due(now_ms),
                    ScreenId::FleetRadar => state.fleet_radar.poll_due(now_ms),
                    _ => false,
                }
            };
            if should_poll {
                if matches!(state.ui_variant, UiVariant::Disruptive) {
                    state.fleet_radar.mark_polled(now_ms);
                } else {
                    match state.active_screen() {
                        ScreenId::Monitor => state.monitor.mark_polled(now_ms),
                        ScreenId::FleetRadar => state.fleet_radar.mark_polled(now_ms),
                        _ => {}
                    }
                }
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
        Action::TimeCursor(time_action) => {
            handle_time_cursor_action(&mut state, time_action);
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
            if let ScreenId::ClusterCanvas { id } = &screen {
                ensure_cluster_canvas_focus(state, id);
            }
            if matches!(screen, ScreenId::Monitor) {
                state.monitor.mark_polled(state.now_ms);
            } else if matches!(screen, ScreenId::FleetRadar) {
                state.fleet_radar.mark_polled(state.now_ms);
            }
            state.screen_stack.push(screen.clone());
            queue_navigation_effects(&screen, effects);
        }
        NavigationAction::Pop => {
            if state.screen_stack.len() > 1 {
                cleanup_active_screen(state, effects);
                state.screen_stack.pop();
                if let Some(active) = state.screen_stack.last() {
                    if matches!(active, ScreenId::Monitor) {
                        state.monitor.mark_polled(state.now_ms);
                    } else if matches!(active, ScreenId::FleetRadar) {
                        state.fleet_radar.mark_polled(state.now_ms);
                    }
                    queue_navigation_effects(active, effects);
                }
            }
        }
        NavigationAction::ReplaceTop(screen) => {
            cleanup_active_screen(state, effects);
            seed_agent_role_for_navigation(state, &screen);
            state.ensure_screen_state(&screen);
            if let ScreenId::ClusterCanvas { id } = &screen {
                ensure_cluster_canvas_focus(state, id);
            }
            if matches!(screen, ScreenId::Monitor) {
                state.monitor.mark_polled(state.now_ms);
            } else if matches!(screen, ScreenId::FleetRadar) {
                state.fleet_radar.mark_polled(state.now_ms);
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

    apply_spine_defaults_for_screen(state);
    refresh_spine_hint(state);
    refresh_spine_completion(state);
    sync_temporal_focus(state);
}

fn ensure_cluster_canvas_focus(state: &mut AppState, id: &str) {
    let Some(cluster_state) = state.clusters.get(id) else {
        return;
    };
    let Some(topology) = cluster_state.topology.as_ref() else {
        return;
    };
    if let Some(canvas_state) = state.cluster_canvases.get_mut(id) {
        canvas_state.update_layout(topology);
    }
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

    let role = state.clusters.get(cluster_id).and_then(|cluster_state| {
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
        } => {
            effects.push(Effect::Backend(BackendRequest::SubscribeClusterLogs {
                cluster_id: cluster_id.clone(),
                agent_id: Some(agent_id.clone()),
            }));
        }
        ScreenId::AgentMicroscope {
            cluster_id,
            agent_id,
        } => {
            effects.push(Effect::Backend(BackendRequest::SubscribeClusterLogs {
                cluster_id: cluster_id.clone(),
                agent_id: Some(agent_id.clone()),
            }));
            effects.push(Effect::Backend(BackendRequest::SubscribeClusterTimeline {
                cluster_id: cluster_id.clone(),
            }));
        }
        ScreenId::Launcher | ScreenId::IntentConsole => {}
    }
}

fn handle_screen_action(state: &mut AppState, action: ScreenAction, effects: &mut Vec<Effect>) {
    match action {
        ScreenAction::Launcher(action) => handle_launcher_action(state, action, effects),
        ScreenAction::Monitor(action) => handle_monitor_action(state, action, effects),
        ScreenAction::FleetRadar(action) => handle_radar_action(state, action, effects),
        ScreenAction::Cluster { id, action } => handle_cluster_action(state, id, action, effects),
        ScreenAction::ClusterCanvas { id, action } => {
            handle_cluster_canvas_action(state, id, action, effects)
        }
        ScreenAction::Agent {
            cluster_id,
            agent_id,
            action,
        } => handle_agent_action(state, cluster_id, agent_id, action, effects),
    }
}

fn handle_launcher_action(
    state: &mut AppState,
    action: launcher::Action,
    effects: &mut Vec<Effect>,
) {
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

fn handle_radar_action(state: &mut AppState, action: radar::Action, _effects: &mut Vec<Effect>) {
    match action {
        radar::Action::MoveSelection { direction, speed } => {
            if state
                .fleet_radar
                .move_selection_direction(state.now_ms, direction, speed)
            {
                sync_camera_to_selection(state);
            }
        }
        radar::Action::CenterOnSelection => {
            sync_camera_to_selection(state);
        }
        radar::Action::ResetView => {
            state.camera = Camera::default();
            state.camera_target = state.camera.position;
            state.camera_velocity = (0.0, 0.0);
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
            let entry = state.clusters.entry(id).or_default();
            entry.cycle_focus(direction);
        }
        cluster::Action::MoveFocused(delta) => {
            let entry = state.clusters.entry(id).or_default();
            entry.move_focused(delta);
        }
        cluster::Action::ActivateFocused => {
            let (agent_id, role) = {
                let entry = state.clusters.entry(id.clone()).or_default();
                let agent_id = entry.activate_focused();
                let role = agent_id.as_ref().and_then(|selected| {
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
                let entry = state.clusters.entry(id.clone()).or_default();
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

fn handle_cluster_canvas_action(
    state: &mut AppState,
    id: String,
    action: cluster_canvas::Action,
    effects: &mut Vec<Effect>,
) {
    match action {
        cluster_canvas::Action::MoveFocus { direction, speed } => {
            let entry = state.cluster_canvases.entry(id).or_default();
            entry.move_focus(direction, speed);
        }
        cluster_canvas::Action::ZoomIn => {
            let agent_id = state
                .cluster_canvases
                .get(&id)
                .and_then(|entry| entry.focused_agent_id());
            if let Some(agent_id) = agent_id {
                let role = state.clusters.get(&id).and_then(|entry| {
                    entry
                        .agents
                        .iter()
                        .find(|agent| agent.id == agent_id)
                        .and_then(|agent| agent.role.clone())
                });
                seed_agent_role(state, &id, &agent_id, role);
                apply_navigation(
                    state,
                    NavigationAction::Push(ScreenId::AgentMicroscope {
                        cluster_id: id,
                        agent_id,
                    }),
                    effects,
                );
            }
        }
    }
}

fn seed_agent_role(state: &mut AppState, cluster_id: &str, agent_id: &str, role: Option<String>) {
    if let Some(role) = role {
        let key = AgentKey::new(cluster_id.to_string(), agent_id.to_string());
        let entry = state.agents.entry(key.clone()).or_default();
        if entry.role.is_none() {
            entry.role = Some(role.clone());
        }
        let microscope_entry = state.agent_microscopes.entry(key).or_default();
        if microscope_entry.role.is_none() {
            microscope_entry.role = Some(role);
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
    let entry = state.agents.entry(key).or_default();
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
        BackendAction::ConnectionFailed(message) => {
            handle_backend_connection_failed(state, message)
        }
        BackendAction::BackendExited(exit) => handle_backend_exited(state, exit),
        BackendAction::Notification(notification) => {
            handle_backend_notification(state, notification)
        }
        BackendAction::ClustersListed(clusters) => handle_clusters_listed(state, clusters),
        BackendAction::ClusterMetricsListed { metrics } => {
            handle_cluster_metrics_listed(state, metrics)
        }
        BackendAction::ClusterSummary { summary } => handle_cluster_summary(state, summary),
        BackendAction::ClusterTopology {
            cluster_id,
            topology,
        } => handle_cluster_topology(state, cluster_id, topology),
        BackendAction::ClusterTopologyError {
            cluster_id,
            message,
        } => handle_cluster_topology_error(state, cluster_id, message),
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
            let latest_ts = params.lines.iter().map(|line| line.timestamp).max();
            let lines = params.lines;
            let dropped_count = params.dropped_count;
            let role_from_lines = lines.iter().find_map(|line| line.role.clone());
            if let Some((key, entry)) = state.agent_microscopes.iter_mut().find(|(_, entry)| {
                entry.log_subscription.as_deref() == Some(params.subscription_id.as_str())
            }) {
                if entry.role.is_none() {
                    if let Some(role) = role_from_lines.clone() {
                        entry.role = Some(role);
                    }
                }
                entry.push_log_lines(lines, dropped_count);
                if let Some(role) = role_from_lines {
                    let entry = state.agents.entry(key.clone()).or_default();
                    if entry.role.is_none() {
                        entry.role = Some(role);
                    }
                }
                advance_time_cursor_if_live(state, latest_ts);
                return;
            }

            if let Some(entry) = state.agents.values_mut().find(|agent| {
                agent.log_subscription.as_deref() == Some(params.subscription_id.as_str())
            }) {
                if entry.role.is_none() {
                    if let Some(role) = role_from_lines.clone() {
                        entry.role = Some(role);
                    }
                }
                entry.push_log_lines(lines, dropped_count);
                advance_time_cursor_if_live(state, latest_ts);
                return;
            }

            if let Some(entry) = state.clusters.get_mut(&params.cluster_id) {
                if entry.log_subscription.as_deref() == Some(params.subscription_id.as_str()) {
                    entry.push_log_lines(lines, dropped_count);
                    advance_time_cursor_if_live(state, latest_ts);
                }
            }
        }
        BackendNotification::ClusterTimelineEvents(params) => {
            let latest_ts = params.events.iter().map(|event| event.timestamp).max();
            let entry = state.clusters.entry(params.cluster_id).or_default();
            entry.push_timeline_events(params.events);
            advance_time_cursor_if_live(state, latest_ts);
        }
        BackendNotification::Unknown { method, .. } => {
            state.last_error = Some(format!("Unhandled backend notification: {method}"));
        }
    }
}

fn advance_time_cursor_if_live(state: &mut AppState, latest_ts: Option<i64>) {
    if state.time_cursor.mode != TimeCursorMode::Live {
        return;
    }
    let Some(latest_ts) = latest_ts else {
        return;
    };
    if latest_ts > state.time_cursor.t_ms {
        state.time_cursor.t_ms = latest_ts;
    }
}

fn handle_clusters_listed(state: &mut AppState, clusters: Vec<crate::protocol::ClusterSummary>) {
    let radar_clusters = clusters.clone();
    state.monitor.set_clusters(clusters, state.now_ms);
    state.fleet_radar.set_clusters(radar_clusters, state.now_ms);
    sync_camera_to_selection(state);
    let ids: HashSet<String> = state
        .monitor
        .clusters
        .iter()
        .map(|cluster| cluster.id.clone())
        .collect();
    state.metrics.retain(|id, _| ids.contains(id));
}

fn sync_camera_to_selection(state: &mut AppState) {
    if let Some(layout) = state.fleet_radar.selected_layout(state.now_ms) {
        state.camera_target = (layout.x as f32, layout.y as f32);
        state.camera_velocity = (0.0, 0.0);
    }
}

fn update_radar_camera_smoothing(state: &mut AppState, dt_ms: i64) {
    let (position, velocity) = step_spring_f32(
        state.camera.position,
        state.camera_velocity,
        state.camera_target,
        dt_ms,
        CAMERA_ACCEL,
        CAMERA_FRICTION,
    );
    state.camera.position = position;
    state.camera_velocity = velocity;

    let dx = state.camera.position.0 - state.camera_target.0;
    let dy = state.camera.position.1 - state.camera_target.1;
    if dx.abs() <= CAMERA_SNAP_EPSILON && dy.abs() <= CAMERA_SNAP_EPSILON {
        state.camera.position = state.camera_target;
        state.camera_velocity = (0.0, 0.0);
    }
}

fn handle_cluster_metrics_listed(state: &mut AppState, metrics: Vec<ClusterMetrics>) {
    for metric in metrics {
        state.metrics.insert(metric.id.clone(), metric);
    }
}

fn handle_cluster_summary(state: &mut AppState, summary: crate::protocol::ClusterSummary) {
    let entry = state.clusters.entry(summary.id.clone()).or_default();
    entry.summary = Some(summary);
}

fn handle_cluster_topology(
    state: &mut AppState,
    cluster_id: String,
    topology: crate::protocol::ClusterTopology,
) {
    let canvas_entry = state
        .cluster_canvases
        .entry(cluster_id.clone())
        .or_default();
    canvas_entry.update_layout(&topology);

    let entry = state.clusters.entry(cluster_id).or_default();
    entry.topology = Some(topology);
    entry.topology_error = None;
}

fn handle_cluster_topology_error(state: &mut AppState, cluster_id: String, message: String) {
    let entry = state.clusters.entry(cluster_id.clone()).or_default();
    entry.topology = None;
    entry.topology_error = Some(message);

    if let Some(canvas_entry) = state.cluster_canvases.get_mut(&cluster_id) {
        canvas_entry.clear_layout();
    }
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
            let active_is_microscope = matches!(
                state.active_screen(),
                ScreenId::AgentMicroscope { cluster_id, agent_id }
                    if cluster_id == &key.cluster_id && agent_id == &key.agent_id
            );
            if active_is_microscope {
                let entry = state.agent_microscopes.entry(key).or_default();
                entry.log_subscription = Some(subscription_id);
            } else {
                let entry = state.agents.entry(key).or_default();
                entry.log_subscription = Some(subscription_id);
            }
        }
        None => {
            let entry = state.clusters.entry(cluster_id.clone()).or_default();
            entry.log_subscription = Some(subscription_id.clone());
            if let Some(canvas_entry) = state.cluster_canvases.get_mut(&cluster_id) {
                canvas_entry.log_subscription = Some(subscription_id);
            }
        }
    }
}

fn handle_cluster_timeline_subscription(
    state: &mut AppState,
    cluster_id: String,
    subscription_id: String,
) {
    let entry = state.clusters.entry(cluster_id.clone()).or_default();
    entry.timeline_subscription = Some(subscription_id.clone());
    if let Some(canvas_entry) = state.cluster_canvases.get_mut(&cluster_id) {
        canvas_entry.timeline_subscription = Some(subscription_id);
    }
}

fn handle_guidance_result(
    state: &mut AppState,
    cluster_id: String,
    agent_id: String,
    result: crate::protocol::GuidanceDeliveryResult,
) {
    let key = AgentKey::new(cluster_id, agent_id);
    let entry = state.agents.entry(key).or_default();
    entry.apply_guidance_result(result);
}

fn handle_guidance_error(
    state: &mut AppState,
    cluster_id: String,
    agent_id: String,
    message: String,
) {
    let key = AgentKey::new(cluster_id, agent_id);
    let entry = state.agents.entry(key).or_default();
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
    apply_spine_defaults_for_screen(state);
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

fn spine_idle(state: &SpineState) -> bool {
    matches!(state.mode, SpineMode::Intent)
        && state.input.input.is_empty()
        && state.completion.is_none()
}

fn apply_spine_defaults_for_screen(state: &mut AppState) {
    if matches!(state.active_screen(), ScreenId::AgentMicroscope { .. }) && spine_idle(&state.spine)
    {
        state.spine.mode = SpineMode::WhisperAgent;
    }
}

fn refresh_spine_hint(state: &mut AppState) {
    state.spine.hint = compute_spine_hint(state);
}

fn refresh_spine_completion(state: &mut AppState) {
    state.spine.completion = build_spine_completion(
        state.spine.mode,
        state.spine.input.input.as_str(),
        state.spine.input.cursor,
    );
}

fn sync_temporal_focus(state: &mut AppState) {
    if !state.temporal_focus.is_active() {
        return;
    }
    state.temporal_focus = state.temporal_focus_scope().unwrap_or(TemporalFocus::None);
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
        ZoomStackContext::FleetRadar => match state.active_screen() {
            ScreenId::Monitor => state.monitor.selected_cluster_id(),
            _ => state.fleet_radar.selected_cluster_id(),
        },
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
    let mut should_refresh_completion = false;
    match action {
        SpineAction::SetMode(mode) => {
            state.spine.mode = mode;
            should_refresh_hint = true;
            should_refresh_completion = true;
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
            should_refresh_completion = true;
        }
        SpineAction::Cancel => {
            reset_spine_state(state);
            should_refresh_hint = true;
            should_refresh_completion = true;
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
            should_refresh_completion = true;
        }
        SpineAction::AcceptCompletion => {
            if let Some(completion) = state.spine.completion.take() {
                if !completion.ghost.is_empty() {
                    state.spine.input.input.push_str(completion.ghost.as_str());
                    state.spine.input.cursor = state.spine.input.input.chars().count();
                }
            }
            should_refresh_hint = true;
            should_refresh_completion = true;
        }
        SpineAction::CycleCompletion => {
            if let Some(completion) = state.spine.completion.as_ref() {
                if completion.candidates.len() > 1 {
                    let next = (completion.selected + 1) % completion.candidates.len();
                    state.spine.completion = select_spine_completion(
                        state.spine.mode,
                        state.spine.input.input.as_str(),
                        state.spine.input.cursor,
                        next,
                    );
                }
            }
        }
        SpineAction::InsertChar(ch) => {
            state.spine.input.insert_char(ch);
            should_refresh_hint = true;
            should_refresh_completion = true;
        }
        SpineAction::Backspace => {
            state.spine.input.backspace();
            should_refresh_hint = true;
            should_refresh_completion = true;
        }
        SpineAction::Delete => {
            state.spine.input.delete();
            should_refresh_hint = true;
            should_refresh_completion = true;
        }
        SpineAction::MoveCursorLeft => {
            state.spine.input.move_left();
            should_refresh_completion = true;
        }
        SpineAction::MoveCursorRight => {
            state.spine.input.move_right();
            should_refresh_completion = true;
        }
        SpineAction::MoveCursorHome => {
            state.spine.input.move_home();
            should_refresh_completion = true;
        }
        SpineAction::MoveCursorEnd => {
            state.spine.input.move_end();
            should_refresh_completion = true;
        }
        SpineAction::Clear => {
            state.spine.input.clear();
            state.spine.completion = None;
            should_refresh_hint = true;
            should_refresh_completion = true;
        }
    }

    if should_refresh_completion {
        refresh_spine_completion(state);
    }
    if should_refresh_hint {
        refresh_spine_hint(state);
    }
}

fn handle_time_cursor_action(state: &mut AppState, action: TimeCursorAction) {
    if !state.temporal_focus.is_active() {
        match action {
            TimeCursorAction::ToggleFollow => {
                let Some(scope) = state.temporal_focus_scope() else {
                    return;
                };
                state.temporal_focus = scope;
            }
            _ => return,
        }
    }

    let bounds = time_bounds_for_focus(state);
    match action {
        TimeCursorAction::Step { delta_ms } => {
            let Some((min, max)) = bounds else {
                return;
            };
            let next = state.time_cursor.t_ms.saturating_add(delta_ms);
            state.time_cursor.t_ms = next.clamp(min, max);
            state.time_cursor.mode = TimeCursorMode::Scrub;
        }
        TimeCursorAction::JumpToLive => {
            state.time_cursor.mode = TimeCursorMode::Live;
            if let Some((_, max)) = bounds {
                state.time_cursor.t_ms = max;
            }
        }
        TimeCursorAction::ToggleFollow => {
            if matches!(state.time_cursor.mode, TimeCursorMode::Live) {
                state.time_cursor.mode = TimeCursorMode::Scrub;
            } else {
                state.time_cursor.mode = TimeCursorMode::Live;
                if let Some((_, max)) = bounds {
                    state.time_cursor.t_ms = max;
                }
            }
        }
    }

    if let Some((min, max)) = bounds {
        state.time_cursor.t_ms = state.time_cursor.t_ms.clamp(min, max);
    }

    if matches!(state.time_cursor.mode, TimeCursorMode::Live) {
        state.temporal_focus = TemporalFocus::None;
    }
}

fn time_bounds_for_focus(state: &AppState) -> Option<(i64, i64)> {
    match &state.temporal_focus {
        TemporalFocus::None => None,
        TemporalFocus::Cluster { id } => time_bounds_for_cluster(state, id, None),
        TemporalFocus::Agent {
            cluster_id,
            agent_id,
        } => time_bounds_for_agent_microscope(state, cluster_id, agent_id)
            .or_else(|| time_bounds_for_cluster(state, cluster_id, Some(agent_id.as_str()))),
    }
}

fn time_bounds_for_agent_microscope(
    state: &AppState,
    cluster_id: &str,
    agent_id: &str,
) -> Option<(i64, i64)> {
    let key = AgentKey::new(cluster_id.to_string(), agent_id.to_string());
    let entry = state.agent_microscopes.get(&key)?;
    let mut min: Option<i64> = None;
    let mut max: Option<i64> = None;
    for line in entry.logs_time.iter() {
        min = Some(min.map_or(line.timestamp, |value| value.min(line.timestamp)));
        max = Some(max.map_or(line.timestamp, |value| value.max(line.timestamp)));
    }
    match (min, max) {
        (Some(min), Some(max)) => Some((min, max)),
        _ => None,
    }
}

fn time_bounds_for_cluster(
    state: &AppState,
    cluster_id: &str,
    agent_id: Option<&str>,
) -> Option<(i64, i64)> {
    let cluster_state = state.clusters.get(cluster_id)?;
    let mut min: Option<i64> = None;
    let mut max: Option<i64> = None;
    let update = |ts: i64, min: &mut Option<i64>, max: &mut Option<i64>| {
        *min = Some(min.map_or(ts, |value| value.min(ts)));
        *max = Some(max.map_or(ts, |value| value.max(ts)));
    };

    for line in cluster_state.logs_time.iter() {
        if let Some(agent_id) = agent_id {
            let matches_agent =
                line.agent.as_deref() == Some(agent_id) || line.sender.as_deref() == Some(agent_id);
            if !matches_agent {
                continue;
            }
        }
        update(line.timestamp, &mut min, &mut max);
    }

    if agent_id.is_none() {
        for event in cluster_state.timeline_time.iter() {
            update(event.timestamp, &mut min, &mut max);
        }
    }

    match (min, max) {
        (Some(min), Some(max)) => Some((min, max)),
        _ => None,
    }
}

fn build_guidance_message(prefix: Option<&str>, message: &str) -> String {
    let trimmed = message.trim();
    match prefix {
        Some(prefix) if trimmed.is_empty() => prefix.to_string(),
        Some(prefix) => format!("{prefix} {trimmed}"),
        None => trimmed.to_string(),
    }
}

fn resolve_canvas_focused_agent(state: &AppState, cluster_id: &str) -> Option<String> {
    let canvas_state = state.cluster_canvases.get(cluster_id)?;
    let focused_id = canvas_state.focused_id.as_deref()?;
    if let Some(layout) = canvas_state.layout.as_ref() {
        if let Some(node) = layout.nodes.get(focused_id) {
            if matches!(node.kind, cluster_canvas::NodeKind::Agent) {
                return Some(node.id.clone());
            }
        }
    }
    let cluster_state = state.clusters.get(cluster_id)?;
    let topology = cluster_state.topology.as_ref()?;
    if topology.agents.iter().any(|agent| agent.id == focused_id) {
        Some(focused_id.to_string())
    } else {
        None
    }
}

fn resolve_focus_target(state: &AppState) -> Option<FocusTarget> {
    match state.active_screen() {
        ScreenId::Agent {
            cluster_id,
            agent_id,
        }
        | ScreenId::AgentMicroscope {
            cluster_id,
            agent_id,
        } => Some(FocusTarget::Agent {
            cluster_id: cluster_id.clone(),
            agent_id: agent_id.clone(),
        }),
        ScreenId::ClusterCanvas { id } => {
            if let Some(agent_id) = resolve_canvas_focused_agent(state, id) {
                return Some(FocusTarget::Agent {
                    cluster_id: id.clone(),
                    agent_id,
                });
            }
            Some(FocusTarget::Cluster { id: id.clone() })
        }
        ScreenId::Cluster { id } => Some(FocusTarget::Cluster { id: id.clone() }),
        ScreenId::Monitor => state
            .monitor
            .selected_cluster_id()
            .map(|id| FocusTarget::Cluster { id }),
        ScreenId::FleetRadar => state
            .fleet_radar
            .selected_cluster_id()
            .map(|id| FocusTarget::Cluster { id }),
        ScreenId::Launcher | ScreenId::IntentConsole => None,
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
        CommandAction::SendGuidance { message, prefix } => {
            let Some(target) = resolve_focus_target(state) else {
                state.toast = Some(ToastState {
                    message: "No focused cluster or agent.".to_string(),
                    level: ToastLevel::Error,
                    expires_at_ms: state.now_ms.saturating_add(TOAST_DURATION_MS),
                });
                return;
            };
            let built = build_guidance_message(prefix.as_deref(), &message);
            if built.trim().is_empty() {
                state.toast = Some(ToastState {
                    message: "Guidance text is required.".to_string(),
                    level: ToastLevel::Error,
                    expires_at_ms: state.now_ms.saturating_add(TOAST_DURATION_MS),
                });
                return;
            }
            let toast_message = match &target {
                FocusTarget::Cluster { id } => {
                    effects.push(Effect::Backend(BackendRequest::SendGuidanceToCluster {
                        cluster_id: id.clone(),
                        message: built.clone(),
                    }));
                    format!("Guidance sent to cluster {id}.")
                }
                FocusTarget::Agent {
                    cluster_id,
                    agent_id,
                } => {
                    effects.push(Effect::Backend(BackendRequest::SendGuidanceToAgent {
                        cluster_id: cluster_id.clone(),
                        agent_id: agent_id.clone(),
                        message: built.clone(),
                    }));
                    format!("Guidance sent to agent {agent_id} @ {cluster_id}.")
                }
            };
            state.toast = Some(ToastState {
                message: toast_message,
                level: ToastLevel::Success,
                expires_at_ms: state.now_ms.saturating_add(TOAST_DURATION_MS),
            });
        }
        CommandAction::TogglePin => {
            if !matches!(state.ui_variant, UiVariant::Disruptive) {
                state.toast = Some(ToastState {
                    message: "Pinning is only available in Disruptive UI.".to_string(),
                    level: ToastLevel::Error,
                    expires_at_ms: state.now_ms.saturating_add(TOAST_DURATION_MS),
                });
                return;
            }
            let Some(target) = resolve_focus_target(state) else {
                state.toast = Some(ToastState {
                    message: "No focus target to pin.".to_string(),
                    level: ToastLevel::Error,
                    expires_at_ms: state.now_ms.saturating_add(TOAST_DURATION_MS),
                });
                return;
            };
            let message = if state.pinned_target.as_ref() == Some(&target) {
                state.pinned_target = None;
                format!("Unpinned {}.", target.label())
            } else {
                state.pinned_target = Some(target.clone());
                format!("Pinned {}.", target.label())
            };
            state.toast = Some(ToastState {
                message,
                level: ToastLevel::Success,
                expires_at_ms: state.now_ms.saturating_add(TOAST_DURATION_MS),
            });
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
        }) => cleanup_agent_subscriptions(state, &cluster_id, &agent_id, effects),
        Some(ScreenId::AgentMicroscope {
            cluster_id,
            agent_id,
        }) => {
            cleanup_agent_subscriptions(state, &cluster_id, &agent_id, effects);
            cleanup_cluster_timeline_subscription(state, &cluster_id, effects);
        }
        _ => {}
    }
}

fn cleanup_cluster_subscriptions(state: &mut AppState, id: &str, effects: &mut Vec<Effect>) {
    let Some(entry) = state.clusters.get_mut(id) else {
        return;
    };
    if let Some(subscription_id) = entry.log_subscription.take() {
        effects.push(Effect::Backend(BackendRequest::Unsubscribe {
            subscription_id,
        }));
    }
    if let Some(subscription_id) = entry.timeline_subscription.take() {
        effects.push(Effect::Backend(BackendRequest::Unsubscribe {
            subscription_id,
        }));
    }
    if let Some(canvas_entry) = state.cluster_canvases.get_mut(id) {
        canvas_entry.log_subscription = None;
        canvas_entry.timeline_subscription = None;
    }
}

fn cleanup_cluster_timeline_subscription(
    state: &mut AppState,
    id: &str,
    effects: &mut Vec<Effect>,
) {
    let Some(entry) = state.clusters.get_mut(id) else {
        return;
    };
    if let Some(subscription_id) = entry.timeline_subscription.take() {
        effects.push(Effect::Backend(BackendRequest::Unsubscribe {
            subscription_id,
        }));
    }
    if let Some(canvas_entry) = state.cluster_canvases.get_mut(id) {
        canvas_entry.timeline_subscription = None;
    }
}

fn cleanup_agent_subscriptions(
    state: &mut AppState,
    cluster_id: &str,
    agent_id: &str,
    effects: &mut Vec<Effect>,
) {
    let key = AgentKey::new(cluster_id.to_string(), agent_id.to_string());
    let mut unsubscribed: HashSet<String> = HashSet::new();
    if let Some(entry) = state.agents.get_mut(&key) {
        if let Some(subscription_id) = entry.log_subscription.take() {
            unsubscribed.insert(subscription_id.clone());
            effects.push(Effect::Backend(BackendRequest::Unsubscribe {
                subscription_id,
            }));
        }
    }
    if let Some(entry) = state.agent_microscopes.get_mut(&key) {
        if let Some(subscription_id) = entry.log_subscription.take() {
            if !unsubscribed.contains(&subscription_id) {
                effects.push(Effect::Backend(BackendRequest::Unsubscribe {
                    subscription_id,
                }));
            }
        }
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
    use crate::protocol::{
        ClusterLogLine, ClusterLogLinesParams, ClusterSummary, ClusterTimelineEventsParams,
        TimelineEvent,
    };

    fn radar_cluster(id: &str) -> ClusterSummary {
        ClusterSummary {
            id: id.to_string(),
            state: "running".to_string(),
            provider: None,
            created_at: 0,
            agent_count: 1,
            message_count: 0,
            cwd: None,
        }
    }

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
    fn command_guidance_sends_to_agent_with_prefix() {
        let mut state = AppState::default();
        state.screen_stack = vec![ScreenId::Agent {
            cluster_id: "cluster-1".to_string(),
            agent_id: "agent-1".to_string(),
        }];

        let (_state, effects) = update(
            state,
            Action::Command(CommandAction::SendGuidance {
                message: "hi".to_string(),
                prefix: Some("[nudge]".to_string()),
            }),
        );

        assert!(
            effects.contains(&Effect::Backend(BackendRequest::SendGuidanceToAgent {
                cluster_id: "cluster-1".to_string(),
                agent_id: "agent-1".to_string(),
                message: "[nudge] hi".to_string(),
            }))
        );
    }

    #[test]
    fn pin_toggles_pinned_target() {
        let mut state = AppState::default();
        state.ui_variant = UiVariant::Disruptive;
        state.screen_stack = vec![ScreenId::FleetRadar];
        state
            .fleet_radar
            .set_clusters(vec![radar_cluster("cluster-1")], 0);

        let (state, _) = update(state, Action::Command(CommandAction::TogglePin));
        assert_eq!(
            state.pinned_target,
            Some(FocusTarget::Cluster {
                id: "cluster-1".to_string()
            })
        );

        let (state, _) = update(state, Action::Command(CommandAction::TogglePin));
        assert_eq!(state.pinned_target, None);
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
    fn spine_accept_completion_appends_ghost() {
        let mut state = AppState::default();
        state.spine.mode = SpineMode::Command;

        let (state, _) = update(state, Action::Spine(SpineAction::InsertChar('p')));
        let completion = state.spine.completion.as_ref().expect("completion");
        assert_eq!(completion.ghost, "rovider");

        let (state, _) = update(state, Action::Spine(SpineAction::AcceptCompletion));
        assert_eq!(state.spine.input.input, "provider");
        assert!(state.spine.completion.is_none());
    }

    #[test]
    fn spine_cycle_completion_updates_ghost() {
        let mut state = AppState::default();
        state.spine.mode = SpineMode::Command;

        let (state, _) = update(state, Action::Spine(SpineAction::InsertChar('i')));
        let completion = state.spine.completion.as_ref().expect("completion");
        assert_eq!(completion.ghost, "ssue");

        let (state, _) = update(state, Action::Spine(SpineAction::CycleCompletion));
        let completion = state.spine.completion.as_ref().expect("completion");
        assert_eq!(completion.ghost, "nterrupt");
    }

    #[test]
    fn spine_cancel_resets_mode_and_input() {
        let mut state = AppState::default();
        state.spine.mode = SpineMode::Command;
        state.spine.input.input = "help".to_string();
        state.spine.input.cursor = 4;
        state.spine.completion = Some(SpineCompletion {
            candidates: vec!["help".to_string()],
            selected: 0,
            ghost: "er".to_string(),
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
        assert!(
            effects.contains(&Effect::Backend(BackendRequest::StartClusterFromText {
                text: "launch".to_string(),
                provider_override: None,
            }))
        );
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
        assert!(
            effects.contains(&Effect::Backend(BackendRequest::SendGuidanceToCluster {
                cluster_id: "cluster-1".to_string(),
                message: "ping".to_string(),
            }))
        );
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
        assert!(
            effects.contains(&Effect::Backend(BackendRequest::SendGuidanceToAgent {
                cluster_id: "cluster-1".to_string(),
                agent_id: "agent-1".to_string(),
                message: "ping".to_string(),
            }))
        );
        assert_eq!(state.spine.mode, SpineMode::Intent);
        assert_eq!(state.spine.input.input, "");
    }

    #[test]
    fn navigation_to_microscope_sets_spine_mode() {
        let state = AppState::default();
        let (state, _) = update(
            state,
            Action::Navigate(NavigationAction::Push(ScreenId::AgentMicroscope {
                cluster_id: "cluster-1".to_string(),
                agent_id: "agent-1".to_string(),
            })),
        );
        assert_eq!(state.spine.mode, SpineMode::WhisperAgent);
    }

    #[test]
    fn navigation_to_microscope_subscribes_timeline() {
        let state = AppState::default();
        let (_, effects) = update(
            state,
            Action::Navigate(NavigationAction::Push(ScreenId::AgentMicroscope {
                cluster_id: "cluster-1".to_string(),
                agent_id: "agent-1".to_string(),
            })),
        );
        assert!(
            effects.contains(&Effect::Backend(BackendRequest::SubscribeClusterTimeline {
                cluster_id: "cluster-1".to_string(),
            }))
        );
    }

    #[test]
    fn spine_submit_whisper_agent_sends_guidance_from_microscope() {
        let mut state = AppState::default();
        state.screen_stack = vec![ScreenId::AgentMicroscope {
            cluster_id: "cluster-1".to_string(),
            agent_id: "agent-1".to_string(),
        }];
        state.spine.mode = SpineMode::WhisperAgent;
        state.spine.input.input = "ping".to_string();
        state.spine.input.cursor = 4;

        let (state, effects) = update(state, Action::Spine(SpineAction::Submit));
        assert!(
            effects.contains(&Effect::Backend(BackendRequest::SendGuidanceToAgent {
                cluster_id: "cluster-1".to_string(),
                agent_id: "agent-1".to_string(),
                message: "ping".to_string(),
            }))
        );
        assert_eq!(state.spine.mode, SpineMode::WhisperAgent);
        assert_eq!(state.spine.input.input, "");
    }

    #[test]
    fn time_cursor_live_updates() {
        let mut state = AppState::default();
        state.time_cursor.mode = TimeCursorMode::Live;
        state.time_cursor.t_ms = 10;

        let cluster_id = "cluster-1".to_string();
        let subscription_id = "sub-logs".to_string();
        state
            .clusters
            .entry(cluster_id.clone())
            .or_default()
            .log_subscription = Some(subscription_id.clone());

        handle_backend_notification(
            &mut state,
            BackendNotification::ClusterLogLines(ClusterLogLinesParams {
                subscription_id,
                cluster_id: cluster_id.clone(),
                lines: vec![
                    ClusterLogLine {
                        id: "line-1".to_string(),
                        timestamp: 100,
                        text: "hello".to_string(),
                        agent: None,
                        role: None,
                        sender: None,
                    },
                    ClusterLogLine {
                        id: "line-2".to_string(),
                        timestamp: 150,
                        text: "world".to_string(),
                        agent: None,
                        role: None,
                        sender: None,
                    },
                ],
                dropped_count: None,
            }),
        );
        assert_eq!(state.time_cursor.t_ms, 150);

        handle_backend_notification(
            &mut state,
            BackendNotification::ClusterTimelineEvents(ClusterTimelineEventsParams {
                subscription_id: "sub-timeline".to_string(),
                cluster_id,
                events: vec![TimelineEvent {
                    id: "event-1".to_string(),
                    timestamp: 175,
                    topic: "ISSUE_OPENED".to_string(),
                    label: "opened".to_string(),
                    approved: None,
                    sender: None,
                }],
            }),
        );
        assert_eq!(state.time_cursor.t_ms, 175);
    }

    #[test]
    fn time_cursor_scrub_does_not_update() {
        let mut state = AppState::default();
        state.time_cursor.mode = TimeCursorMode::Scrub;
        state.time_cursor.t_ms = 200;

        let cluster_id = "cluster-2".to_string();
        let subscription_id = "sub-logs".to_string();
        state
            .clusters
            .entry(cluster_id.clone())
            .or_default()
            .log_subscription = Some(subscription_id.clone());

        handle_backend_notification(
            &mut state,
            BackendNotification::ClusterLogLines(ClusterLogLinesParams {
                subscription_id,
                cluster_id,
                lines: vec![ClusterLogLine {
                    id: "line-3".to_string(),
                    timestamp: 500,
                    text: "late".to_string(),
                    agent: None,
                    role: None,
                    sender: None,
                }],
                dropped_count: None,
            }),
        );
        assert_eq!(state.time_cursor.t_ms, 200);
    }

    fn sample_log_line(id: &str, timestamp: i64) -> ClusterLogLine {
        ClusterLogLine {
            id: id.to_string(),
            timestamp,
            text: "log".to_string(),
            agent: None,
            role: None,
            sender: None,
        }
    }

    #[test]
    fn time_cursor_step_clamps_and_enters_scrub() {
        let mut state = AppState::default();
        state.temporal_focus = TemporalFocus::Cluster {
            id: "cluster-1".to_string(),
        };
        state.time_cursor.mode = TimeCursorMode::Live;
        state.time_cursor.t_ms = 200;

        let mut cluster_state = cluster::State::default();
        cluster_state.push_log_lines(
            vec![sample_log_line("l1", 100), sample_log_line("l2", 200)],
            None,
        );
        state
            .clusters
            .insert("cluster-1".to_string(), cluster_state);

        let (state, _) = update(
            state,
            Action::TimeCursor(TimeCursorAction::Step {
                delta_ms: -TIME_SCRUB_STEP_MS,
            }),
        );
        assert_eq!(state.time_cursor.mode, TimeCursorMode::Scrub);
        assert_eq!(state.time_cursor.t_ms, 100);
    }

    #[test]
    fn time_cursor_jump_and_toggle_follow() {
        let mut state = AppState::default();
        state.screen_stack = vec![ScreenId::ClusterCanvas {
            id: "cluster-2".to_string(),
        }];
        state.temporal_focus = TemporalFocus::Cluster {
            id: "cluster-2".to_string(),
        };
        state.time_cursor.mode = TimeCursorMode::Scrub;
        state.time_cursor.t_ms = 120;

        let mut cluster_state = cluster::State::default();
        cluster_state.push_log_lines(
            vec![sample_log_line("l1", 100), sample_log_line("l2", 250)],
            None,
        );
        state
            .clusters
            .insert("cluster-2".to_string(), cluster_state);

        let (state, _) = update(state, Action::TimeCursor(TimeCursorAction::JumpToLive));
        assert_eq!(state.time_cursor.mode, TimeCursorMode::Live);
        assert_eq!(state.time_cursor.t_ms, 250);

        let (state, _) = update(state, Action::TimeCursor(TimeCursorAction::ToggleFollow));
        assert_eq!(state.time_cursor.mode, TimeCursorMode::Scrub);

        let (state, _) = update(state, Action::TimeCursor(TimeCursorAction::ToggleFollow));
        assert_eq!(state.time_cursor.mode, TimeCursorMode::Live);
        assert_eq!(state.time_cursor.t_ms, 250);
    }

    #[test]
    fn time_cursor_large_step_clamps_to_max() {
        let mut state = AppState::default();
        state.temporal_focus = TemporalFocus::Cluster {
            id: "cluster-3".to_string(),
        };
        state.time_cursor.mode = TimeCursorMode::Scrub;
        state.time_cursor.t_ms = 150;

        let mut cluster_state = cluster::State::default();
        cluster_state.push_log_lines(
            vec![sample_log_line("l1", 100), sample_log_line("l2", 220)],
            None,
        );
        state
            .clusters
            .insert("cluster-3".to_string(), cluster_state);

        let (state, _) = update(
            state,
            Action::TimeCursor(TimeCursorAction::Step {
                delta_ms: TIME_SCRUB_STEP_LARGE_MS,
            }),
        );
        assert_eq!(state.time_cursor.t_ms, 220);
    }

    #[test]
    fn radar_camera_centering() {
        let mut state = AppState::default();
        state.ui_variant = UiVariant::Disruptive;
        state.now_ms = 10_000;
        state.fleet_radar.set_clusters(
            vec![radar_cluster("west"), radar_cluster("east")],
            state.now_ms,
        );
        state
            .fleet_radar
            .layout_angles
            .insert("west".to_string(), std::f64::consts::PI);
        state
            .fleet_radar
            .layout_angles
            .insert("east".to_string(), 0.0);
        state.fleet_radar.selected = 0;

        let (state, _) = update(
            state,
            Action::Screen(ScreenAction::FleetRadar(radar::Action::CenterOnSelection)),
        );
        assert!(state.camera_target.0 < 0.0);

        let (state, _) = update(
            state,
            Action::Screen(ScreenAction::FleetRadar(radar::Action::MoveSelection {
                direction: radar::Direction::Right,
                speed: radar::MoveSpeed::Step,
            })),
        );
        assert_eq!(
            state.fleet_radar.selected_cluster_id().as_deref(),
            Some("east")
        );
        assert!(state.camera_target.0 > 0.0);

        let (state, _) = update(
            state,
            Action::Screen(ScreenAction::FleetRadar(radar::Action::ResetView)),
        );
        assert_eq!(state.camera, Camera::default());
        assert_eq!(state.camera_target, (0.0, 0.0));
        assert_eq!(state.camera_velocity, (0.0, 0.0));
    }
}
