#![allow(clippy::needless_return)]
#![allow(clippy::io_other_error)]

use std::env;
use std::io::{self, stdout};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossterm::event::{self, Event, KeyEventKind};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use zeroshot_tui::app::{
    resolve_ui_variant, Action, AppState, BackendAction, BackendRequest, Effect, InitialScreen,
    StartupOptions,
};
use zeroshot_tui::backend::stdio::StdioBackendClient;
use zeroshot_tui::backend::{BackendClient, BackendConfig, BackendError, BackendEvent};
use zeroshot_tui::commands;
use zeroshot_tui::input;
use zeroshot_tui::terminal::TerminalGuard;
use zeroshot_tui::ui;

type ActionSender = mpsc::Sender<Action>;
type ActionReceiver = mpsc::Receiver<Action>;
type TuiTerminal = Terminal<CrosstermBackend<std::io::Stdout>>;

const INITIAL_SCREEN_ENV: &str = "ZEROSHOT_TUI_INITIAL_SCREEN";
const PROVIDER_OVERRIDE_ENV: &str = "ZEROSHOT_TUI_PROVIDER_OVERRIDE";
const UI_VARIANT_ENV: &str = "ZEROSHOT_TUI_UI";

fn main() -> io::Result<()> {
    if handle_cli_flags()? {
        return Ok(());
    }
    run_app()
}

fn run_app() -> io::Result<()> {
    let guard = init_terminal_guard()?;
    let mut terminal = setup_terminal()?;
    maybe_force_panic();
    let startup_options = parse_startup_options()?;

    let (action_tx, action_rx) = mpsc::channel::<Action>();
    let mut backend = connect_backend(&action_tx)?;
    let mut state = AppState::new();
    state.apply_startup_options(startup_options);
    let tick_rate = Duration::from_millis(250);
    let mut last_tick = Instant::now();

    app_loop(
        &mut terminal,
        &mut state,
        &action_tx,
        &action_rx,
        &mut backend,
        tick_rate,
        &mut last_tick,
    )?;

    shutdown_backend(backend)?;
    drop(terminal);
    guard.restore()?;
    Ok(())
}

fn handle_cli_flags() -> io::Result<bool> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--version") {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return Ok(true);
    }
    if args.iter().any(|arg| arg == "--smoke-test") {
        println!("ok");
        return Ok(true);
    }
    Ok(false)
}

fn init_terminal_guard() -> io::Result<TerminalGuard> {
    let guard = TerminalGuard::new()?;
    guard.install_panic_hook();
    Ok(guard)
}

fn setup_terminal() -> io::Result<TuiTerminal> {
    Terminal::new(CrosstermBackend::new(stdout()))
}

fn maybe_force_panic() {
    if env::var("ZEROSHOT_TUI_PANIC").ok().as_deref() == Some("1") {
        panic!("ZEROSHOT_TUI_PANIC=1 requested");
    }
}

fn parse_startup_options() -> io::Result<StartupOptions> {
    let mut options = StartupOptions::default();
    let mut args = env::args().skip(1);
    let mut ui_arg: Option<String> = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--initial-screen" => {
                let value = args.next().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "--initial-screen requires a value",
                    )
                })?;
                options.initial_screen = Some(parse_initial_screen(&value)?);
            }
            "--provider-override" => {
                let value = args.next().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "--provider-override requires a value",
                    )
                })?;
                if !value.trim().is_empty() {
                    options.provider_override = Some(value.trim().to_string());
                }
            }
            "--ui" => {
                let value = args.next().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "--ui requires a value")
                })?;
                ui_arg = Some(value);
            }
            _ => {}
        }
    }

    if options.initial_screen.is_none() {
        if let Ok(value) = env::var(INITIAL_SCREEN_ENV) {
            if !value.trim().is_empty() {
                options.initial_screen = Some(parse_initial_screen(&value)?);
            }
        }
    }

    if options.provider_override.is_none() {
        if let Ok(value) = env::var(PROVIDER_OVERRIDE_ENV) {
            if !value.trim().is_empty() {
                options.provider_override = Some(value.trim().to_string());
            }
        }
    }

    let env_ui = env::var(UI_VARIANT_ENV).ok();
    options.ui_variant = resolve_ui_variant(ui_arg.as_deref(), env_ui.as_deref())
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;

    Ok(options)
}

fn parse_initial_screen(value: &str) -> io::Result<InitialScreen> {
    InitialScreen::parse(value).map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))
}

fn app_loop(
    terminal: &mut TuiTerminal,
    state: &mut AppState,
    action_tx: &ActionSender,
    action_rx: &ActionReceiver,
    backend: &mut Option<StdioBackendClient>,
    tick_rate: Duration,
    last_tick: &mut Instant,
) -> io::Result<()> {
    loop {
        handle_terminal_events(state, action_tx, tick_rate, last_tick)?;
        drain_actions(state, action_rx, backend, action_tx)?;
        terminal.draw(|frame| ui::render(frame, state))?;

        if state.should_quit {
            break;
        }
    }

    Ok(())
}

fn handle_terminal_events(
    state: &AppState,
    action_tx: &ActionSender,
    tick_rate: Duration,
    last_tick: &mut Instant,
) -> io::Result<()> {
    let timeout = tick_rate.saturating_sub(last_tick.elapsed());
    if event::poll(timeout)? {
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                if let Some(action) = input::route_key(state, key) {
                    send_action(action_tx, action)?;
                }
            }
            Event::Resize(width, height) => {
                send_action(action_tx, Action::Resize { width, height })?;
            }
            _ => {}
        }
    }

    if last_tick.elapsed() >= tick_rate {
        send_action(action_tx, Action::Tick { now_ms: now_ms() })?;
        *last_tick = Instant::now();
    }

    Ok(())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn drain_actions(
    state: &mut AppState,
    action_rx: &ActionReceiver,
    backend: &mut Option<StdioBackendClient>,
    action_tx: &ActionSender,
) -> io::Result<()> {
    loop {
        match action_rx.try_recv() {
            Ok(action) => {
                let (next_state, effects) =
                    zeroshot_tui::app::update(std::mem::take(state), action);
                *state = next_state;
                execute_effects(effects, backend, action_tx)?;
            }
            Err(mpsc::TryRecvError::Empty) => break,
            Err(mpsc::TryRecvError::Disconnected) => {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "action channel disconnected",
                ));
            }
        }
    }

    Ok(())
}

fn connect_backend(action_tx: &ActionSender) -> io::Result<Option<StdioBackendClient>> {
    match StdioBackendClient::connect(BackendConfig::default()) {
        Ok(mut client) => {
            if let Some(events) = client.take_event_receiver() {
                let tx = action_tx.clone();
                thread::spawn(move || handle_backend_events(events, tx));
            }
            send_action(action_tx, Action::Backend(BackendAction::Connected))?;
            Ok(Some(client))
        }
        Err(err) => {
            send_action(
                action_tx,
                Action::Backend(BackendAction::ConnectionFailed(err.to_string())),
            )?;
            Ok(None)
        }
    }
}

fn handle_backend_events(events: mpsc::Receiver<BackendEvent>, action_tx: ActionSender) {
    for event in events {
        let action = match event {
            BackendEvent::Notification(notification) => {
                Action::Backend(BackendAction::Notification(notification))
            }
            BackendEvent::BackendExited(exit) => {
                Action::Backend(BackendAction::BackendExited(exit))
            }
        };

        if !send_action_thread(&action_tx, action) {
            break;
        }
    }
}

fn send_action(action_tx: &ActionSender, action: Action) -> io::Result<()> {
    action_tx.send(action).map_err(|err| {
        io::Error::new(
            io::ErrorKind::BrokenPipe,
            format!("action channel closed: {err}"),
        )
    })
}

fn send_action_thread(action_tx: &ActionSender, action: Action) -> bool {
    if let Err(err) = action_tx.send(action) {
        eprintln!("backend event send failed: {err}");
        return false;
    }
    true
}

fn execute_effects(
    effects: Vec<Effect>,
    backend: &mut Option<StdioBackendClient>,
    action_tx: &ActionSender,
) -> io::Result<()> {
    for effect in effects {
        match effect {
            Effect::Backend(request) => {
                if let Some(client) = backend.as_ref() {
                    match execute_backend_request(client, request) {
                        Ok(Some(action)) => {
                            send_action(action_tx, Action::Backend(action))?;
                        }
                        Ok(None) => {}
                        Err(err) => {
                            send_action(
                                action_tx,
                                Action::Backend(BackendAction::Error(err.to_string())),
                            )?;
                        }
                    }
                } else {
                    send_action(
                        action_tx,
                        Action::Backend(BackendAction::ConnectionFailed(
                            "Backend unavailable".to_string(),
                        )),
                    )?;
                }
            }
            Effect::Command(request) => match commands::dispatch(request) {
                Ok(actions) => {
                    for action in actions {
                        send_action(action_tx, action)?;
                    }
                }
                Err(err) => {
                    send_action(
                        action_tx,
                        Action::Backend(BackendAction::Error(err.to_string())),
                    )?;
                }
            },
        }
    }

    Ok(())
}

fn execute_backend_request(
    client: &StdioBackendClient,
    request: BackendRequest,
) -> Result<Option<BackendAction>, BackendError> {
    match request {
        BackendRequest::ListClusters => list_clusters(client),
        BackendRequest::ListClusterMetrics { cluster_ids } => {
            list_cluster_metrics(client, cluster_ids)
        }
        BackendRequest::GetClusterSummary { cluster_id } => get_cluster_summary(client, cluster_id),
        BackendRequest::GetClusterTopology { cluster_id } => {
            get_cluster_topology(client, cluster_id)
        }
        BackendRequest::SubscribeClusterLogs {
            cluster_id,
            agent_id,
        } => subscribe_cluster_logs(client, cluster_id, agent_id),
        BackendRequest::SubscribeClusterTimeline { cluster_id } => {
            subscribe_cluster_timeline(client, cluster_id)
        }
        BackendRequest::StartClusterFromText {
            text,
            provider_override,
        } => start_cluster_from_text(client, text, provider_override),
        BackendRequest::StartClusterFromIssue {
            reference,
            provider_override,
        } => start_cluster_from_issue(client, reference, provider_override),
        BackendRequest::SendGuidanceToCluster {
            cluster_id,
            message,
        } => send_guidance_to_cluster(client, cluster_id, message),
        BackendRequest::SendGuidanceToAgent {
            cluster_id,
            agent_id,
            message,
        } => send_guidance_to_agent(client, cluster_id, agent_id, message),
        BackendRequest::Unsubscribe { subscription_id } => unsubscribe(client, subscription_id),
    }
}

fn list_clusters(client: &StdioBackendClient) -> Result<Option<BackendAction>, BackendError> {
    let result = client.list_clusters()?;
    Ok(Some(BackendAction::ClustersListed(result.clusters)))
}

fn list_cluster_metrics(
    client: &StdioBackendClient,
    cluster_ids: Option<Vec<String>>,
) -> Result<Option<BackendAction>, BackendError> {
    let result = client
        .list_cluster_metrics(zeroshot_tui::protocol::ListClusterMetricsParams { cluster_ids })?;
    Ok(Some(BackendAction::ClusterMetricsListed {
        metrics: result.metrics,
    }))
}

fn get_cluster_summary(
    client: &StdioBackendClient,
    cluster_id: String,
) -> Result<Option<BackendAction>, BackendError> {
    let result = client
        .get_cluster_summary(zeroshot_tui::protocol::GetClusterSummaryParams { cluster_id })?;
    Ok(Some(BackendAction::ClusterSummary {
        summary: result.summary,
    }))
}

fn get_cluster_topology(
    client: &StdioBackendClient,
    cluster_id: String,
) -> Result<Option<BackendAction>, BackendError> {
    match client.get_cluster_topology(zeroshot_tui::protocol::GetClusterTopologyParams {
        cluster_id: cluster_id.clone(),
    }) {
        Ok(result) => Ok(Some(BackendAction::ClusterTopology {
            cluster_id,
            topology: result.topology,
        })),
        Err(err) => Ok(Some(BackendAction::ClusterTopologyError {
            cluster_id,
            message: err.to_string(),
        })),
    }
}

fn subscribe_cluster_logs(
    client: &StdioBackendClient,
    cluster_id: String,
    agent_id: Option<String>,
) -> Result<Option<BackendAction>, BackendError> {
    let result =
        client.subscribe_cluster_logs(zeroshot_tui::protocol::SubscribeClusterLogsParams {
            cluster_id: cluster_id.clone(),
            agent_id: agent_id.clone(),
        })?;
    Ok(Some(BackendAction::SubscribedClusterLogs {
        cluster_id,
        agent_id,
        subscription_id: result.subscription_id,
    }))
}

fn subscribe_cluster_timeline(
    client: &StdioBackendClient,
    cluster_id: String,
) -> Result<Option<BackendAction>, BackendError> {
    let result = client.subscribe_cluster_timeline(
        zeroshot_tui::protocol::SubscribeClusterTimelineParams {
            cluster_id: cluster_id.clone(),
        },
    )?;
    Ok(Some(BackendAction::SubscribedClusterTimeline {
        cluster_id,
        subscription_id: result.subscription_id,
    }))
}

fn start_cluster_from_text(
    client: &StdioBackendClient,
    text: String,
    provider_override: Option<String>,
) -> Result<Option<BackendAction>, BackendError> {
    let result =
        client.start_cluster_from_text(zeroshot_tui::protocol::StartClusterFromTextParams {
            text,
            provider_override,
            cluster_id: None,
        })?;
    Ok(Some(BackendAction::StartClusterResult {
        cluster_id: result.cluster_id,
    }))
}

fn start_cluster_from_issue(
    client: &StdioBackendClient,
    reference: String,
    provider_override: Option<String>,
) -> Result<Option<BackendAction>, BackendError> {
    let result =
        client.start_cluster_from_issue(zeroshot_tui::protocol::StartClusterFromIssueParams {
            r#ref: reference,
            provider_override,
            cluster_id: None,
        })?;
    Ok(Some(BackendAction::StartClusterResult {
        cluster_id: result.cluster_id,
    }))
}

fn send_guidance_to_cluster(
    client: &StdioBackendClient,
    cluster_id: String,
    message: String,
) -> Result<Option<BackendAction>, BackendError> {
    client.send_guidance_to_cluster(zeroshot_tui::protocol::SendGuidanceToClusterParams {
        cluster_id,
        text: message,
        timeout_ms: None,
    })?;
    Ok(None)
}

fn send_guidance_to_agent(
    client: &StdioBackendClient,
    cluster_id: String,
    agent_id: String,
    message: String,
) -> Result<Option<BackendAction>, BackendError> {
    match client.send_guidance_to_agent(zeroshot_tui::protocol::SendGuidanceToAgentParams {
        cluster_id: cluster_id.clone(),
        agent_id: agent_id.clone(),
        text: message,
        timeout_ms: None,
    }) {
        Ok(result) => Ok(Some(BackendAction::GuidanceToAgentResult {
            cluster_id,
            agent_id,
            result: result.result,
        })),
        Err(err) => Ok(Some(BackendAction::GuidanceToAgentError {
            cluster_id,
            agent_id,
            message: err.to_string(),
        })),
    }
}

fn unsubscribe(
    client: &StdioBackendClient,
    subscription_id: String,
) -> Result<Option<BackendAction>, BackendError> {
    client.unsubscribe(zeroshot_tui::protocol::UnsubscribeParams { subscription_id })?;
    Ok(None)
}

fn shutdown_backend(mut backend: Option<StdioBackendClient>) -> io::Result<()> {
    if let Some(mut backend) = backend.take() {
        backend.shutdown().map_err(|err| {
            io::Error::new(
                io::ErrorKind::Other,
                format!("backend shutdown failed: {err}"),
            )
        })?;
    }

    Ok(())
}
