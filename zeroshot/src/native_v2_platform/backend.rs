use super::*;

mod lineage;
use lineage::selected_part;

impl NativeV2PlatformController {
    pub(super) async fn admit_resume_runtime(
        &self,
        saved: &AdmittedRun,
        runtime: Option<openengine_cluster_protocol::RunResumeRuntime>,
        submission_key: openengine_cluster_protocol::IdempotencyKey,
    ) -> Result<AdmittedRun, NativeV2PlatformError> {
        let mut admitted = saved.clone();
        if let Some(runtime) = runtime {
            let submission = crate::native_v2_runtime_config::replacement_submission(
                &admitted,
                runtime,
                submission_key,
            )?;
            self.validate_harnesses(&submission.runtime)?;
            let mut admission = crate::native_v2_admission::NativeV2AdmissionContext::new(
                &self.harnesses,
                self.delivery_policy,
            );
            admission.checkpointless_requests = self.checkpointless_requests;
            admission.hosted_workspace_checkpoint_opt_out =
                self.hosted_workspace_checkpoint_opt_out;
            if let Some(catalog) = self.run_sizes.as_deref() {
                admission = admission.with_run_sizes(catalog);
            }
            admitted = NativeV2Admission
                .admit_with_context(submission, admission)
                .await?;
            crate::v2_run_ledger::validate_admitted_run(&admitted)?;
        }
        if let Some(catalog) = self.run_sizes.as_deref() {
            crate::native_v2_admission::resolve_runtime_size(&mut admitted.runtime, catalog)?;
        }
        self.validate_harnesses(&admitted.runtime)?;
        Ok(admitted)
    }
}

#[async_trait]
impl ClusterBackend for NativeV2PlatformController {
    async fn initialize(
        &self,
        _context: &ConnectionContext,
        _params: InitializeParams,
    ) -> Result<InitializeResult, BackendError> {
        let graph_profiles = GraphProfileSet::new(vec![GraphProfile::Full])
            .map_err(|_| BackendError::new(INTERNAL_ERROR_CODE, "invalid capability set"))?;
        Ok(InitializeResult::new(
            ServerCapabilities {
                graph_profiles,
                logs: true,
                agent_attach: true,
            },
            ClusterStatus::empty(),
        ))
    }

    async fn get(
        &self,
        _context: &ConnectionContext,
        _params: GetParams,
    ) -> Result<GetResult, BackendError> {
        Ok(GetResult::empty())
    }

    async fn run_submit(
        &self,
        _context: &ConnectionContext,
        params: RunSubmitParams,
    ) -> Result<RunSubmitResult, BackendError> {
        let receipt = self.submit(params).await.map_err(platform_backend_error)?;
        let family = self
            .run_lineage(&receipt.run_id)
            .await
            .map_err(platform_backend_error)?;
        Ok(RunSubmitResult {
            run_id: family.root().clone(),
            current_part_id: family.current().clone(),
        })
    }

    async fn run_list(
        &self,
        _context: &ConnectionContext,
        _params: RunListParams,
    ) -> Result<RunListResult, BackendError> {
        let families = self.run_families().await.map_err(platform_backend_error)?;
        let mut runs = Vec::with_capacity(families.len());
        for family in families {
            let mut result = self
                .projected_status(&family, None)
                .await
                .map_err(platform_backend_error)?;
            result.steering = Default::default();
            runs.push(result);
        }
        Ok(RunListResult { runs })
    }

    async fn run_status(
        &self,
        _context: &ConnectionContext,
        params: RunStatusParams,
    ) -> Result<RunStatusResult, BackendError> {
        let family = self
            .run_lineage(&params.run_id)
            .await
            .map_err(platform_backend_error)?;
        self.projected_status(&family, params.part_id.as_ref())
            .await
            .map_err(platform_backend_error)
    }

    async fn run_watch(
        &self,
        _context: &ConnectionContext,
        params: RunWatchParams,
    ) -> Result<(RunWatchResult, RunWatchEventStream), BackendError> {
        let (result, source) = self.watch(params).await.map_err(platform_backend_error)?;
        Ok((result, RunSubscriptionStream::new(WatchSource(source))))
    }

    async fn run_logs(
        &self,
        _context: &ConnectionContext,
        params: RunLogsParams,
    ) -> Result<(RunLogsResult, RunLogEventStream), BackendError> {
        let (result, source) = self.logs(params).await.map_err(platform_backend_error)?;
        Ok((result, RunSubscriptionStream::new(LogsSource(source))))
    }

    async fn run_attach(
        &self,
        _context: &ConnectionContext,
        params: RunAttachParams,
    ) -> Result<(RunAttachResult, RunAttachEventStream), BackendError> {
        let (family, part) = self
            .read_target(&params.run_id, params.part_id.as_ref())
            .await?;
        let (mut result, source) = self
            .attach(RunAttachParams {
                run_id: part,
                ..params
            })
            .await
            .map_err(platform_backend_error)?;
        result.run_id = family.root().clone();
        Ok((
            result,
            RunSubscriptionStream::new(AttachSource {
                source,
                run_id: family.root().clone(),
            }),
        ))
    }

    async fn run_force(
        &self,
        _context: &ConnectionContext,
        params: RunForceParams,
    ) -> Result<RunForceResult, BackendError> {
        let (family, run_id) = self
            .current_target(&params.run_id, params.part_id.as_ref())
            .await?;
        let mut result = self
            .force(RunForceParams { run_id, ..params })
            .await
            .map_err(platform_backend_error)?;
        result.run_id = family.root().clone();
        result.current_part_id = family.current().clone();
        family.project_recovery(family.current(), &mut result.workspace_recovery);
        self.aggregate_status(&family, &mut result.status, &mut result.at_cursor)
            .await
            .map_err(platform_backend_error)?;
        Ok(result)
    }

    async fn run_steer(
        &self,
        _context: &ConnectionContext,
        params: openengine_cluster_protocol::RunSteerParams,
    ) -> Result<openengine_cluster_protocol::RunSteerResult, BackendError> {
        let (family, run_id) = self
            .current_target(&params.run_id, params.part_id.as_ref())
            .await?;
        let mut result = self
            .steer(openengine_cluster_protocol::RunSteerParams { run_id, ..params })
            .await
            .map_err(platform_backend_error)?;
        result.run_id = family.root().clone();
        Ok(result)
    }

    async fn run_suspend(
        &self,
        _context: &ConnectionContext,
        params: openengine_cluster_protocol::RunSuspendParams,
    ) -> Result<openengine_cluster_protocol::RunSuspendResult, BackendError> {
        let (family, run_id) = self
            .current_target(&params.run_id, params.part_id.as_ref())
            .await?;
        let mut result = self
            .suspend(RunStatusParams { run_id, ..params })
            .await
            .map_err(platform_backend_error)?;
        result.run_id = family.root().clone();
        result.current_part_id = family.current().clone();
        family.project_recovery(family.current(), &mut result.workspace_recovery);
        self.aggregate_status(&family, &mut result.status, &mut result.at_cursor)
            .await
            .map_err(platform_backend_error)?;
        Ok(result)
    }

    async fn inbox_list(
        &self,
        _context: &ConnectionContext,
        params: openengine_cluster_protocol::InboxListParams,
    ) -> Result<openengine_cluster_protocol::InboxListResult, BackendError> {
        self.inbox_list(params)
            .await
            .map_err(platform_backend_error)
    }
    async fn inbox_show(
        &self,
        _context: &ConnectionContext,
        params: openengine_cluster_protocol::InboxShowParams,
    ) -> Result<openengine_cluster_protocol::InboxShowResult, BackendError> {
        let family = self
            .run_lineage(&params.run_id)
            .await
            .map_err(platform_backend_error)?;
        let selected =
            selected_part(&family, params.part_id.as_ref()).map_err(platform_backend_error)?;
        for id in family
            .parts()
            .iter()
            .rev()
            .filter(|id| params.part_id.is_none() || *id == selected)
        {
            match self.inbox_part(id, family.successor(id).cloned()).await {
                Ok(mut request) => {
                    request.run_id = family.root().clone();
                    return Ok(request);
                }
                Err(NativeV2PlatformError::Ledger(RunLedgerError::VectorNotFound)) => continue,
                Err(error) => return Err(platform_backend_error(error)),
            }
        }
        Err(platform_backend_error(
            RunLedgerError::VectorNotFound.into(),
        ))
    }

    async fn run_vectors(
        &self,
        _context: &ConnectionContext,
        params: openengine_cluster_protocol::RunVectorsParams,
    ) -> Result<openengine_cluster_protocol::RunVectorsResult, BackendError> {
        let (family, run_id) = self
            .read_target(&params.run_id, params.part_id.as_ref())
            .await?;
        let mut result = self
            .vectors(openengine_cluster_protocol::RunVectorsParams { run_id, ..params })
            .await
            .map_err(platform_backend_error)?;
        result.run_id = family.root().clone();
        for vector in &mut result.vectors {
            vector.run_id = family.root().clone();
        }
        Ok(result)
    }

    async fn run_vector(
        &self,
        _context: &ConnectionContext,
        params: openengine_cluster_protocol::RunVectorParams,
    ) -> Result<openengine_cluster_protocol::RunVectorResult, BackendError> {
        let (family, run_id) = self
            .read_target(&params.run_id, params.part_id.as_ref())
            .await?;
        let mut result = self
            .vector(openengine_cluster_protocol::RunVectorParams { run_id, ..params })
            .await
            .map_err(platform_backend_error)?;
        result.run_id = family.root().clone();
        Ok(result)
    }

    async fn run_checkpoints(
        &self,
        _context: &ConnectionContext,
        params: openengine_cluster_protocol::RunCheckpointsParams,
    ) -> Result<openengine_cluster_protocol::RunCheckpointsResult, BackendError> {
        let (family, run_id) = self
            .read_target(&params.run_id, params.part_id.as_ref())
            .await?;
        let mut result = self
            .checkpoints(openengine_cluster_protocol::RunCheckpointsParams { run_id, ..params })
            .await
            .map_err(platform_backend_error)?;
        result.run_id = family.root().clone();
        Ok(result)
    }

    async fn run_resume_requirements(
        &self,
        _context: &ConnectionContext,
        params: openengine_cluster_protocol::RunResumeRequirementsParams,
    ) -> Result<openengine_cluster_protocol::RunResumeRequirementsResult, BackendError> {
        let (_, run_id) = self
            .current_target(&params.run_id, params.part_id.as_ref())
            .await?;
        let source = self
            .recoverable_resume_source(&run_id)
            .await
            .map_err(platform_backend_error)?;
        let submission = crate::native_v2_runtime_config::replacement_submission(
            &source.admitted,
            params.runtime,
            source.submission_key,
        )
        .map_err(|error| platform_backend_error(error.into()))?;
        let connections = submission
            .connection_requirements()
            .into_iter()
            .map(|(key, fields)| (key, fields.into_iter().collect()))
            .collect();
        Ok(openengine_cluster_protocol::RunResumeRequirementsResult { connections })
    }

    async fn run_resume(
        &self,
        _context: &ConnectionContext,
        params: RunResumeParams,
    ) -> Result<RunResumeResult, BackendError> {
        let (family, run_id) = self
            .current_target(&params.run_id, Some(&params.part_id))
            .await?;
        let mut result = self
            .resume(RunResumeParams { run_id, ..params })
            .await
            .map_err(platform_backend_error)?;
        result.run_id = family.root().clone();
        Ok(result)
    }

    async fn run_discard_workspace(
        &self,
        _context: &ConnectionContext,
        params: RunDiscardWorkspaceParams,
    ) -> Result<RunDiscardWorkspaceResult, BackendError> {
        let (family, run_id) = self
            .current_target(&params.run_id, params.part_id.as_ref())
            .await?;
        let mut result = self
            .discard_workspace(RunDiscardWorkspaceParams { run_id, ..params })
            .await
            .map_err(platform_backend_error)?;
        result.run_id = family.root().clone();
        Ok(result)
    }
}

struct WatchSource(RunWatchSubscription);

#[async_trait]
impl RunSubscriptionSource<RunWatchEventNotification> for WatchSource {
    async fn next(&mut self) -> Option<RunSubscriptionItem<RunWatchEventNotification>> {
        Some(durable_item(self.0.recv().await))
    }
}

struct LogsSource(RunLogsSubscription);

#[async_trait]
impl RunSubscriptionSource<RunLogEventNotification> for LogsSource {
    async fn next(&mut self) -> Option<RunSubscriptionItem<RunLogEventNotification>> {
        Some(durable_item(self.0.recv().await))
    }
}

fn durable_item<E>(result: Result<Option<E>, NativeV2ObservationError>) -> RunSubscriptionItem<E> {
    match result {
        Ok(Some(event)) => RunSubscriptionItem::Event(event),
        Ok(None) => RunSubscriptionItem::Closed {
            reason: SubscriptionCloseReason::Done,
        },
        Err(_) => RunSubscriptionItem::Closed {
            reason: SubscriptionCloseReason::SourceUnavailable,
        },
    }
}

struct AttachSource {
    source: RunAttachSubscription,
    run_id: RunId,
}

#[async_trait]
impl RunSubscriptionSource<RunAttachEventNotification> for AttachSource {
    async fn next(&mut self) -> Option<RunSubscriptionItem<RunAttachEventNotification>> {
        match self.source.recv().await {
            Ok(mut event) => {
                event.run_id = self.run_id.clone();
                Some(RunSubscriptionItem::Event(event))
            }
            Err(NativeV2ObservationError::AttachLagged) => Some(RunSubscriptionItem::Closed {
                reason: SubscriptionCloseReason::SlowConsumer,
            }),
            Err(_) => Some(RunSubscriptionItem::Closed {
                reason: SubscriptionCloseReason::Done,
            }),
        }
    }
}

fn platform_backend_error(error: NativeV2PlatformError) -> BackendError {
    match error {
        NativeV2PlatformError::Ledger(
            error @ RunLedgerError::AdmittedRunTooLarge {
                submission_bytes,
                input_bytes,
            },
        ) => BackendError::invalid_params(
            "submission.too_large",
            error.to_string(),
            Some(
                serde_json::json!({"submissionBytes":submission_bytes,"inputBytes":input_bytes,"limitBytes":crate::v2_run_ledger::MAX_ADMITTED_RUN_BYTES}),
            ),
        ),
        NativeV2PlatformError::Admission(error) => {
            BackendError::invalid_params(GRAPH_INVALID, error.to_string(), None)
        }
        NativeV2PlatformError::Allocation(CapsuleAllocationUnavailable::SourceCheckout) => {
            BackendError::application(
                "source_checkout_unavailable",
                "source checkout failed: repository access, branch, or revision is unavailable",
                None,
            )
        }
        NativeV2PlatformError::Allocation(CapsuleAllocationUnavailable::ContextCheckout) => {
            BackendError::application(
                "context_checkout_unavailable",
                "context repository checkout failed; inspect the operator diagnostic",
                None,
            )
        }
        NativeV2PlatformError::Environment(error) => match &error {
            RunEnvironmentError::MissingConnection(key)
            | RunEnvironmentError::MissingField(key, _) => BackendError::application(
                "connection_unavailable",
                error.to_string(),
                Some(serde_json::json!({ "connection": key.as_str() })),
            ),
            _ => BackendError::application(SCHEMA_VIOLATION, error.to_string(), None),
        },
        NativeV2PlatformError::Ledger(RunLedgerError::SubmissionConflict { existing_run_id }) => {
            BackendError::application(
                IDEMPOTENCY_REUSE,
                "submission key identifies another run",
                Some(serde_json::json!({ "runId": existing_run_id })),
            )
        }
        NativeV2PlatformError::StalePart(current) => BackendError::application(
            openengine_cluster_protocol::RUN_CONFLICT,
            format!(
                "the current part is {}; refresh the run before acting",
                current.as_str()
            ),
            Some(serde_json::json!({"currentPartId":current})),
        ),
        NativeV2PlatformError::InvalidRequest(message) => {
            BackendError::application("invalid_request", message, None)
        }
        NativeV2PlatformError::Supervisor(NativeV2SupervisorError::Ledger(
            RunLedgerError::InvalidEvent(message),
        )) => BackendError::application("invalid_request", message, None),
        NativeV2PlatformError::Ledger(RunLedgerError::VectorNotFound) => {
            BackendError::application(NOT_FOUND, "vector was not found", None)
        }
        NativeV2PlatformError::Ledger(RunLedgerError::RunNotFound)
        | NativeV2PlatformError::Observation(NativeV2ObservationError::RunNotFound) => {
            BackendError::application(NOT_FOUND, "run was not found", None)
        }
        NativeV2PlatformError::Observation(NativeV2ObservationError::ExecutionNotFound) => {
            BackendError::application(NOT_FOUND, "execution was not found", None)
        }
        NativeV2PlatformError::Observation(
            NativeV2ObservationError::ExecutionNotActive
            | NativeV2ObservationError::ExecutionNotLive,
        ) => BackendError::application(GONE, "execution is no longer active", None),
        NativeV2PlatformError::Observation(NativeV2ObservationError::Ledger(
            RunLedgerError::Storage | RunLedgerError::SqliteStorage(_) | RunLedgerError::Corrupt,
        )) => BackendError::application(
            "SOURCE_UNAVAILABLE",
            "durable run history is unavailable",
            None,
        ),
        _ => BackendError::new(INTERNAL_ERROR_CODE, "native-v2 operation failed"),
    }
}

pub(crate) fn submission_digest(
    submission: &RunSubmission,
) -> Result<Sha256Digest, NativeV2PlatformError> {
    let bytes =
        serde_json::to_vec(submission).map_err(|_| NativeV2PlatformError::SubmissionIdentity)?;
    Sha256Digest::new(format!("{:x}", Sha256::digest(bytes)))
        .map_err(|_| NativeV2PlatformError::SubmissionIdentity)
}

pub(super) async fn append_interrupted(
    ledger: &dyn RunLedger,
    stored: &StoredRun,
    reason: &str,
    outcome: WorkerOutcome,
) -> Result<(), RunLedgerError> {
    let Some(primary) = &stored.snapshot.primary_graph else {
        return append_terminal_failure(ledger, stored, reason, outcome).await;
    };
    if stored.snapshot.terminal.is_some() {
        return Ok(());
    }
    let mut events = Vec::new();
    if primary.auxiliary_pending {
        // Preserve observed counts, but a lost controller cannot account for every billed turn.
        events.push(RunEvent::RunTokenUsageObserved { usage: None });
        events.push(RunEvent::SafeLog {
            execution: None,
            timestamp: crate::native_v2_runner::current_timestamp(),
            stream: crate::v2_run_ledger::SafeLogStream::System,
            line: crate::v2_run_ledger::SafeLogLine::new(
                "Active Memory postmortem interrupted before finalization; primary graph result preserved; auxiliary token usage is incomplete",
            )?,
            diagnostic: None,
        });
    }
    events.push(RunEvent::Terminal {
        result: primary.result.clone(),
    });
    ledger.append(&stored.snapshot.run_id, events).await?;
    Ok(())
}

pub(super) async fn append_terminal_failure(
    ledger: &dyn RunLedger,
    stored: &StoredRun,
    reason: &str,
    outcome: WorkerOutcome,
) -> Result<(), RunLedgerError> {
    if stored.snapshot.terminal.is_some() {
        return Ok(());
    }
    let events = crate::native_v2_supervisor::logging::terminal_failure_events(
        &stored.snapshot,
        reason,
        outcome,
    )?;
    ledger.append(&stored.snapshot.run_id, events).await?;
    Ok(())
}

#[cfg(test)]
#[path = "backend/backend_error_tests.rs"]
mod backend_error_tests;
