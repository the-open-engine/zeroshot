use super::*;

#[derive(Clone, Default)]
pub(super) struct ActivityRegistry {
    state: Arc<Mutex<ActivityState>>,
}

#[derive(Default)]
struct ActivityState {
    active: BTreeMap<ActiveKey, ActiveInvocation>,
    closed_runs: BTreeSet<RunId>,
    closed: bool,
}

struct ActiveInvocation {
    cancel: watch::Sender<bool>,
    binding: Option<SessionBinding>,
    done: watch::Sender<bool>,
}

pub(super) struct ActivityCloseTarget {
    pub(super) binding: Option<SessionBinding>,
    pub(super) done: watch::Receiver<bool>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ActiveKey {
    run_id: RunId,
    execution: ExecutionId,
}

pub(super) struct ActivityToken {
    registry: ActivityRegistry,
    key: ActiveKey,
}

impl ActivityRegistry {
    pub(super) async fn register(
        &self,
        reference: &ExecutionRef,
        cancel: watch::Sender<bool>,
    ) -> Result<ActivityToken, NodeRunnerError> {
        let key = ActiveKey {
            run_id: reference.run_id.clone(),
            execution: reference.execution,
        };
        let (done, _) = watch::channel(false);
        let mut state = self.state.lock().await;
        if state.closed || state.closed_runs.contains(&key.run_id) {
            return Err(NodeRunnerError::RunClosed);
        }
        match state.active.entry(key.clone()) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(ActiveInvocation {
                    cancel,
                    binding: None,
                    done,
                });
            }
            std::collections::btree_map::Entry::Occupied(_) => {
                return Err(NodeRunnerError::ExecutionActive);
            }
        }
        Ok(ActivityToken {
            registry: self.clone(),
            key,
        })
    }

    pub(super) async fn begin_close(&self, run_id: &RunId) -> Vec<ActivityCloseTarget> {
        let targets = {
            let mut state = self.state.lock().await;
            state.closed_runs.insert(run_id.clone());
            state
                .active
                .iter()
                .filter(|(key, _)| key.run_id == *run_id)
                .map(|(_, active)| close_target(active))
                .collect::<Vec<_>>()
        };
        cancel_targets(&targets);
        targets.into_iter().map(|(_, target)| target).collect()
    }

    pub(super) async fn begin_close_all(&self) -> Vec<ActivityCloseTarget> {
        let targets = {
            let mut state = self.state.lock().await;
            state.closed = true;
            state.active.values().map(close_target).collect::<Vec<_>>()
        };
        cancel_targets(&targets);
        targets.into_iter().map(|(_, target)| target).collect()
    }
}

fn close_target(active: &ActiveInvocation) -> (watch::Sender<bool>, ActivityCloseTarget) {
    (
        active.cancel.clone(),
        ActivityCloseTarget {
            binding: active.binding.clone(),
            done: active.done.subscribe(),
        },
    )
}

fn cancel_targets(targets: &[(watch::Sender<bool>, ActivityCloseTarget)]) {
    for (cancel, _) in targets {
        let _ = cancel.send(true);
    }
}

impl ActivityToken {
    pub(super) async fn bind_session(
        &self,
        binding: SessionBinding,
    ) -> Result<(), NodeRunnerError> {
        let mut state = self.registry.state.lock().await;
        if state.closed || state.closed_runs.contains(&self.key.run_id) {
            return Err(NodeRunnerError::RunClosed);
        }
        let active = state
            .active
            .get_mut(&self.key)
            .ok_or(NodeRunnerError::Cancelled)?;
        active.binding = Some(binding);
        Ok(())
    }

    pub(super) async fn finish(self) {
        let active = self.registry.state.lock().await.active.remove(&self.key);
        if let Some(active) = active {
            let _ = active.done.send(true);
        }
    }
}

#[derive(Clone)]
pub(super) struct ManagedSession {
    inner: Arc<dyn NodeSession>,
    closed: Arc<AtomicBool>,
}

impl ManagedSession {
    fn new(inner: Arc<dyn NodeSession>) -> Self {
        Self {
            inner,
            closed: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(super) fn inner(&self) -> Arc<dyn NodeSession> {
        self.inner.clone()
    }

    async fn is_live(&self) -> bool {
        !self.closed.load(Ordering::SeqCst) && self.inner.is_live().await
    }

    async fn close(&self) {
        if !self.closed.swap(true, Ordering::SeqCst) {
            self.inner.close().await;
        }
    }

    fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.closed, &other.closed)
    }
}

#[derive(Clone)]
pub(super) struct SessionPool {
    factory: Arc<dyn SessionFactory>,
    entries: Arc<Mutex<BTreeMap<SessionKey, SessionEntry>>>,
    boundary: ReusableSessionBoundary,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReusableSessionBoundary {
    Run,
    Owner,
}

#[derive(Clone, Copy)]
pub(super) struct SessionCheckout<'a> {
    pub(super) invocation: &'a NodeInvocation,
    pub(super) environment: &'a ResolvedEnvironment,
    pub(super) owner_slot: u64,
}

impl SessionPool {
    pub(super) fn new(factory: Arc<dyn SessionFactory>) -> Self {
        Self {
            factory,
            entries: Arc::new(Mutex::new(BTreeMap::new())),
            boundary: ReusableSessionBoundary::Run,
        }
    }

    pub(super) fn new_owner_scoped(factory: Arc<dyn SessionFactory>) -> Self {
        Self {
            factory,
            entries: Arc::new(Mutex::new(BTreeMap::new())),
            boundary: ReusableSessionBoundary::Owner,
        }
    }

    pub(super) fn provider_session_slot(
        &self,
        invocation: &NodeInvocation,
        owner_slot: u64,
    ) -> Result<NodeInstanceId, NodeRunnerError> {
        match self.boundary {
            ReusableSessionBoundary::Run => Ok(invocation.reference.node_instance),
            ReusableSessionBoundary::Owner => {
                NodeInstanceId::new(owner_slot).map_err(|_| NodeRunnerError::InvalidRole)
            }
        }
    }

    pub(super) async fn checkout(
        &self,
        request: SessionCheckout<'_>,
        cancellation: &mut watch::Receiver<bool>,
    ) -> Result<SessionLease, NodeRunnerError> {
        let scope = session_scope(&request.invocation.binding)?;
        if scope == SessionScope::Execution {
            let session = self
                .open_session(request.invocation, request.environment, cancellation)
                .await?;
            return Ok(SessionLease {
                session,
                pool: self.clone(),
                kind: SessionLeaseKind::Execution,
            });
        }

        let key = self.reusable_key(request.invocation, request.owner_slot)?;
        self.checkout_reusable(key, request, cancellation).await
    }

    fn reusable_key(
        &self,
        invocation: &NodeInvocation,
        owner_slot: u64,
    ) -> Result<SessionKey, NodeRunnerError> {
        Ok(match self.boundary {
            ReusableSessionBoundary::Run => SessionKey::Run {
                run_id: invocation.reference.run_id.clone(),
                node_instance: invocation.reference.node_instance,
            },
            ReusableSessionBoundary::Owner => SessionKey::Owner {
                node: invocation.reference.node.clone(),
                provider_session_slot: self.provider_session_slot(invocation, owner_slot)?,
            },
        })
    }

    async fn checkout_reusable(
        &self,
        key: SessionKey,
        request: SessionCheckout<'_>,
        cancellation: &mut watch::Receiver<bool>,
    ) -> Result<SessionLease, NodeRunnerError> {
        loop {
            match self.checkout_action(&key).await? {
                CheckoutAction::Reuse(session) => {
                    return self.reuse(key, session, cancellation).await;
                }
                CheckoutAction::Wait(mut ready) => {
                    wait_for_open(&mut ready, cancellation).await?;
                }
                CheckoutAction::Open(ready) => {
                    let opened = self
                        .open_session(request.invocation, request.environment, cancellation)
                        .await;
                    return self.finish_open(key, ready, opened).await;
                }
            }
        }
    }

    async fn checkout_action(&self, key: &SessionKey) -> Result<CheckoutAction, NodeRunnerError> {
        let mut entries = self.entries.lock().await;
        match entries.get(key) {
            Some(SessionEntry::Lost) => Err(NodeRunnerError::SessionLost),
            Some(SessionEntry::Live(session)) => Ok(CheckoutAction::Reuse(session.clone())),
            Some(SessionEntry::Opening(ready)) => Ok(CheckoutAction::Wait(ready.subscribe())),
            None | Some(SessionEntry::Replaceable) => {
                let (ready, _) = watch::channel(false);
                let ready = Arc::new(ready);
                entries.insert(key.clone(), SessionEntry::Opening(ready.clone()));
                Ok(CheckoutAction::Open(ready))
            }
        }
    }

    async fn reuse(
        &self,
        key: SessionKey,
        session: ManagedSession,
        cancellation: &mut watch::Receiver<bool>,
    ) -> Result<SessionLease, NodeRunnerError> {
        let live = tokio::select! {
            biased;
            _ = wait_for_cancellation(cancellation) => return Err(NodeRunnerError::Cancelled),
            live = session.is_live() => live,
        };
        if !live {
            self.invalidate(key, session, SessionEntry::Lost).await;
            return Err(NodeRunnerError::SessionLost);
        }
        Ok(SessionLease {
            session,
            pool: self.clone(),
            kind: SessionLeaseKind::Reusable(key),
        })
    }

    async fn open_session(
        &self,
        invocation: &NodeInvocation,
        environment: &ResolvedEnvironment,
        cancellation: &mut watch::Receiver<bool>,
    ) -> Result<ManagedSession, NodeRunnerError> {
        let session = tokio::select! {
            biased;
            _ = wait_for_cancellation(cancellation) => return Err(NodeRunnerError::Cancelled),
            session = self.factory.open(invocation, environment) => session?,
        };
        let session = ManagedSession::new(session);
        let live = tokio::select! {
            biased;
            _ = wait_for_cancellation(cancellation) => {
                session.close().await;
                return Err(NodeRunnerError::Cancelled);
            },
            live = session.is_live() => live,
        };
        if !live {
            session.close().await;
            return Err(NodeRunnerError::SessionOpen);
        }
        Ok(session)
    }

    async fn finish_open(
        &self,
        key: SessionKey,
        ready: Arc<watch::Sender<bool>>,
        opened: Result<ManagedSession, NodeRunnerError>,
    ) -> Result<SessionLease, NodeRunnerError> {
        let mut close = None;
        let cancelled = matches!(&opened, Err(NodeRunnerError::Cancelled));
        let result = {
            let mut entries = self.entries.lock().await;
            let still_opening = matches!(
                entries.get(&key),
                Some(SessionEntry::Opening(current)) if Arc::ptr_eq(current, &ready)
            );
            if !still_opening {
                if let Ok(session) = &opened {
                    close = Some(session.clone());
                }
                Err(if cancelled {
                    NodeRunnerError::Cancelled
                } else {
                    NodeRunnerError::SessionLost
                })
            } else {
                match opened {
                    Ok(session) => {
                        entries.insert(key.clone(), SessionEntry::Live(session.clone()));
                        Ok(SessionLease {
                            session,
                            pool: self.clone(),
                            kind: SessionLeaseKind::Reusable(key),
                        })
                    }
                    Err(error) => {
                        if error == NodeRunnerError::Cancelled {
                            entries.insert(
                                key,
                                match self.boundary {
                                    ReusableSessionBoundary::Run => SessionEntry::Lost,
                                    ReusableSessionBoundary::Owner => SessionEntry::Replaceable,
                                },
                            );
                        } else {
                            entries.remove(&key);
                        }
                        Err(error)
                    }
                }
            }
        };
        let _ = ready.send(true);
        if let Some(session) = close {
            session.close().await;
        }
        result
    }

    async fn invalidate(&self, key: SessionKey, session: ManagedSession, next: SessionEntry) {
        let mut entries = self.entries.lock().await;
        if matches!(entries.get(&key), Some(SessionEntry::Live(current)) if current.same(&session))
        {
            entries.insert(key, next);
        }
        drop(entries);
        session.close().await;
    }

    pub(super) async fn close_run(&self, run_id: &RunId) {
        if self.boundary == ReusableSessionBoundary::Owner {
            return;
        }
        let mut entries = self.entries.lock().await;
        let keys = entries
            .keys()
            .filter(|key| matches!(key, SessionKey::Run { run_id: key_run_id, .. } if key_run_id == run_id))
            .cloned()
            .collect::<Vec<_>>();
        let entries_to_close = keys
            .into_iter()
            .filter_map(|key| match entries.insert(key, SessionEntry::Lost) {
                Some(SessionEntry::Live(session)) => Some(EntryToClose::Session(session)),
                Some(SessionEntry::Opening(ready)) => Some(EntryToClose::Opening(ready)),
                Some(SessionEntry::Replaceable | SessionEntry::Lost) | None => None,
            })
            .collect::<Vec<_>>();
        drop(entries);
        close_entries(entries_to_close).await;
    }

    pub(super) async fn close_bound(&self, binding: SessionBinding, terminal: bool) {
        match binding.kind {
            SessionLeaseKind::Execution => binding.session.close().await,
            SessionLeaseKind::Reusable(key) => {
                let next = if terminal {
                    SessionEntry::Lost
                } else {
                    SessionEntry::Replaceable
                };
                self.invalidate(key, binding.session, next).await;
            }
        }
    }

    pub(super) async fn close_all(&self) {
        let mut entries = self.entries.lock().await;
        let entries_to_close = entries
            .values_mut()
            .filter_map(|entry| {
                let previous = std::mem::replace(entry, SessionEntry::Lost);
                match previous {
                    SessionEntry::Live(session) => Some(EntryToClose::Session(session)),
                    SessionEntry::Opening(ready) => Some(EntryToClose::Opening(ready)),
                    SessionEntry::Replaceable | SessionEntry::Lost => None,
                }
            })
            .collect::<Vec<_>>();
        drop(entries);
        close_entries(entries_to_close).await;
    }
}

async fn close_entries(entries: Vec<EntryToClose>) {
    for entry in entries {
        match entry {
            EntryToClose::Session(session) => session.close().await,
            EntryToClose::Opening(ready) => {
                let _ = ready.send(true);
            }
        }
    }
}

async fn wait_for_ready(receiver: &mut watch::Receiver<bool>) {
    while !*receiver.borrow_and_update() {
        if receiver.changed().await.is_err() {
            return;
        }
    }
}

async fn wait_for_open(
    ready: &mut watch::Receiver<bool>,
    cancellation: &mut watch::Receiver<bool>,
) -> Result<(), NodeRunnerError> {
    tokio::select! {
        biased;
        _ = wait_for_cancellation(cancellation) => Err(NodeRunnerError::Cancelled),
        _ = wait_for_ready(ready) => Ok(()),
    }
}

enum CheckoutAction {
    Reuse(ManagedSession),
    Wait(watch::Receiver<bool>),
    Open(Arc<watch::Sender<bool>>),
}

enum EntryToClose {
    Session(ManagedSession),
    Opening(Arc<watch::Sender<bool>>),
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum SessionKey {
    Run {
        run_id: RunId,
        node_instance: NodeInstanceId,
    },
    Owner {
        node: NodeName,
        provider_session_slot: NodeInstanceId,
    },
}

enum SessionEntry {
    Opening(Arc<watch::Sender<bool>>),
    Live(ManagedSession),
    /// The previous execution invalidated its session, so a reducer-authorized dispatch may
    /// establish a replacement. Passive session loss remains permanently fail-closed.
    Replaceable,
    Lost,
}

pub(super) struct SessionLease {
    pub(super) session: ManagedSession,
    pool: SessionPool,
    kind: SessionLeaseKind,
}

impl SessionLease {
    pub(super) fn binding(&self) -> SessionBinding {
        SessionBinding {
            session: self.session.clone(),
            kind: self.kind.clone(),
        }
    }

    pub(super) async fn finish(self, clean: bool) {
        match self.kind {
            SessionLeaseKind::Execution => self.session.close().await,
            SessionLeaseKind::Reusable(_) if clean => {}
            SessionLeaseKind::Reusable(key) => {
                self.pool
                    .invalidate(key, self.session, SessionEntry::Replaceable)
                    .await;
            }
        }
    }
}

#[derive(Clone)]
enum SessionLeaseKind {
    Execution,
    Reusable(SessionKey),
}

#[derive(Clone)]
pub(super) struct SessionBinding {
    session: ManagedSession,
    kind: SessionLeaseKind,
}
