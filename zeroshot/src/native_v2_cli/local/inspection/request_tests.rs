use super::*;
use openengine_cluster_protocol::{RunAttachResult, RunLogsResult, RunWatchResult, SubscriptionId};
use openengine_cluster_server::native_v2::{
    RunAttachEventStream, RunLogEventStream, RunSubscriptionItem, RunSubscriptionSource,
    RunSubscriptionStream, RunWatchEventStream,
};

const LIMIT: Duration = DEFAULT_READY_TIMEOUT.saturating_add(Duration::from_secs(2));

#[tokio::test]
async fn unreadable_ledger_does_not_retry_an_unanswered_controller() {
    let fixture = Fixture::new(false, false).await;
    let status = fixture
        .backend
        .read_recorded_status(RunStatusParams {
            run_id: fixture.id.clone(),
        })
        .await
        .unwrap();
    let paths = fixture.backend.paths(&fixture.id).unwrap();
    let _endpoint = ServedEndpoint::start(&paths, LiveController::new(None, &status));
    std::fs::write(paths.ledger(), b"not a database").unwrap();
    tokio::time::timeout(LIMIT, async {
        let (status, list) = tokio::join!(
            fixture.backend.status_local(RunStatusParams {
                run_id: fixture.id.clone()
            }),
            fixture.backend.list_local(),
        );
        assert!(matches!(
            status,
            Err(NativeV2CliError::LocalRequestTimeout(_))
        ));
        assert!(list.unwrap().runs.is_empty());
    })
    .await
    .expect("unanswered probes must not be retried through ledger errors");
}

#[cfg(unix)]
#[test]
fn list_keeps_all_runs_with_a_low_descriptor_limit() {
    const STATE: &str = "ZEROSHOT_LIST_DESCRIPTOR_FIXTURE";
    const COUNT: usize = 32;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    if let Some(state) = std::env::var_os(STATE) {
        let backend = LocalCliBackend::new(
            state.into(),
            "unused".into(),
            std::env::current_dir().unwrap(),
            "git".into(),
        );
        let listed =
            runtime.block_on(async { tokio::time::timeout(LIMIT, backend.list_local()).await });
        assert_eq!(listed.unwrap().unwrap().runs.len(), COUNT);
        return;
    }
    runtime.block_on(async {
        let first = Fixture::new(false, false).await;
        let state = first.backend.state_root.clone();
        let mut fixtures = vec![first];
        for _ in 1..COUNT {
            fixtures.push(Fixture::in_state(false, false, Some(state.clone())).await);
        }
        let mut endpoints = Vec::new();
        for fixture in &fixtures {
            let status = fixture
                .backend
                .read_recorded_status(RunStatusParams {
                    run_id: fixture.id.clone(),
                })
                .await
                .unwrap();
            endpoints.push(ServedEndpoint::start(
                &fixture.backend.paths(&fixture.id).unwrap(),
                LiveController::new(None, &status),
            ));
        }
        use std::os::unix::process::CommandExt;
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .arg("list_keeps_all_runs_with_a_low_descriptor_limit")
            .arg("--test-threads=1")
            .env(STATE, state);
        // Only the inspecting child has a small limit; controller endpoints remain independent.
        unsafe {
            command.pre_exec(|| {
                let limit = libc::rlimit {
                    rlim_cur: 64,
                    rlim_max: 64,
                };
                if libc::setrlimit(libc::RLIMIT_NOFILE, &limit) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let output = tokio::process::Command::from(command)
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
    });
}

#[tokio::test]
async fn force_reply_has_one_deadline_and_names_the_queued_request() {
    let fixture = Fixture::new(false, false).await;
    let recorded = fixture
        .backend
        .read_recorded_status(RunStatusParams {
            run_id: fixture.id.clone(),
        })
        .await
        .unwrap();
    let gate = Arc::new(ForceGate::default());
    let controller = Arc::new(LiveController {
        status: None,
        stopped: failed(&recorded, "force_stopped"),
        forced: AtomicBool::new(false),
        force_gate: Some(gate.clone()),
    });
    let _endpoint = ServedEndpoint::start(
        &fixture.backend.paths(&fixture.id).unwrap(),
        controller.clone(),
    );
    let before = tree(fixture.root.path());
    let error = tokio::time::timeout(
        LIMIT,
        fixture.backend.force_local(RunForceParams {
            run_id: fixture.id.clone(),
        }),
    )
    .await
    .expect("force-stop must return within one request deadline")
    .unwrap_err();
    let message = error.to_string();
    assert!(message.contains(fixture.id.as_str()), "{message}");
    assert!(
        message.contains(&std::process::id().to_string()),
        "{message}"
    );
    assert!(message.contains("10 seconds"), "{message}");
    assert!(message.contains("queued"), "{message}");
    assert!(message.contains("answers again"), "{message}");
    assert!(controller.forced.load(Ordering::SeqCst));
    assert_eq!(before, tree(fixture.root.path()));
    gate.release.notify_one();
}

#[tokio::test]
async fn stalled_status_bounds_inspection_resume_and_stream_replay() {
    let fixture = Fixture::new(false, false).await;
    let recorded = fixture
        .backend
        .read_recorded_status(RunStatusParams {
            run_id: fixture.id.clone(),
        })
        .await
        .unwrap();
    let controller = LiveController::new(None, &recorded);
    let _endpoint = ServedEndpoint::start(&fixture.backend.paths(&fixture.id).unwrap(), controller);
    let before = tree(fixture.root.path());
    tokio::time::timeout(LIMIT, async {
        tokio::join!(
            async {
                let status = fixture
                    .backend
                    .status_local(RunStatusParams {
                        run_id: fixture.id.clone(),
                    })
                    .await
                    .unwrap();
                assert_eq!(status.status, recorded.status);
                assert_eq!(status.controller.unwrap().state, ControllerState::Unknown);
            },
            async {
                let status = fixture
                    .backend
                    .run_checkpoints(
                        None,
                        openengine_cluster_protocol::RunCheckpointsParams {
                            run_id: fixture.id.clone(),
                            after: None,
                            limit: None,
                        },
                    )
                    .await
                    .unwrap();
                assert_eq!(status.controller.unwrap().state, ControllerState::Unknown);
            },
            async {
                let error = fixture
                    .backend
                    .run_resume_status(
                        None,
                        RunStatusParams {
                            run_id: fixture.id.clone(),
                        },
                    )
                    .await
                    .unwrap_err();
                assert!(error.to_string().contains("unknown"));
            },
            async {
                let mut stream = fixture
                    .backend
                    .inspect_watch(RunWatchParams {
                        run_id: fixture.id.clone(),
                        from_cursor: None,
                    })
                    .await
                    .unwrap();
                assert!(drain(&mut stream, false, &fixture.id).await > 0);
            },
            async {
                let mut stream = fixture
                    .backend
                    .inspect_logs(RunLogsParams {
                        run_id: fixture.id.clone(),
                        from_cursor: None,
                        execution: None,
                    })
                    .await
                    .unwrap();
                assert!(drain(&mut stream, false, &fixture.id).await > 0);
            },
            async {
                let mut stream = fixture
                    .backend
                    .inspect_attach(RunAttachParams {
                        run_id: fixture.id.clone(),
                        execution: fixture.execution.clone(),
                    })
                    .await
                    .unwrap();
                assert!(drain(&mut stream, false, &fixture.id).await > 0);
            },
        );
    })
    .await
    .expect("all first status requests must be bounded");
    assert_eq!(before, tree(fixture.root.path()));
}

#[tokio::test]
async fn list_probes_all_stalled_controllers_in_one_interval_and_keeps_other_runs() {
    let first = Fixture::new(false, false).await;
    let state = first.backend.state_root.clone();
    let mut fixtures = vec![first];
    for _ in 0..2 {
        fixtures.push(Fixture::in_state(false, false, Some(state.clone())).await);
    }
    let finished = Fixture::in_state(true, false, Some(state.clone())).await;
    let responding = Fixture::in_state(false, false, Some(state)).await;
    let mut endpoints = Vec::new();
    for fixture in fixtures.iter().chain([&responding]) {
        let status = fixture
            .backend
            .read_recorded_status(RunStatusParams {
                run_id: fixture.id.clone(),
            })
            .await
            .unwrap();
        let current = (fixture.id == responding.id).then_some(status.clone());
        endpoints.push(ServedEndpoint::start(
            &fixture.backend.paths(&fixture.id).unwrap(),
            LiveController::new(current, &status),
        ));
    }
    let listed = tokio::time::timeout(LIMIT, fixtures[0].backend.list_local())
        .await
        .expect("list must not wait one deadline per stalled controller")
        .unwrap();
    assert_eq!(listed.runs.len(), 5);
    assert!(
        listed
            .runs
            .windows(2)
            .all(|pair| pair[0].run_id.as_str() < pair[1].run_id.as_str())
    );
    for status in listed.runs {
        if status.run_id == finished.id || status.run_id == responding.id {
            assert!(status.controller.is_none());
        } else {
            assert_eq!(status.controller.unwrap().state, ControllerState::Unknown);
        }
    }
}

struct RequestController {
    status: RunStatusResult,
    status_delay: Duration,
    acknowledge: bool,
    entered: tokio::sync::Notify,
}

struct Quiet;

#[async_trait]
impl<E: Send> RunSubscriptionSource<E> for Quiet {
    async fn next(&mut self) -> Option<RunSubscriptionItem<E>> {
        std::future::pending().await
    }
}

impl RequestController {
    async fn acknowledgement(&self) -> SubscriptionId {
        self.entered.notify_one();
        if !self.acknowledge {
            std::future::pending::<()>().await;
        }
        SubscriptionId::new(uuid::Uuid::now_v7().to_string())
    }
}

#[async_trait]
impl ClusterBackend for RequestController {
    async fn initialize(
        &self,
        _context: &ConnectionContext,
        _params: InitializeParams,
    ) -> Result<InitializeResult, BackendError> {
        Ok(InitializeResult::new(
            ServerCapabilities::default(),
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

    async fn run_status(
        &self,
        _context: &ConnectionContext,
        _params: RunStatusParams,
    ) -> Result<RunStatusResult, BackendError> {
        tokio::time::sleep(self.status_delay).await;
        Ok(self.status.clone())
    }

    async fn run_watch(
        &self,
        _context: &ConnectionContext,
        params: RunWatchParams,
    ) -> Result<(RunWatchResult, RunWatchEventStream), BackendError> {
        Ok((
            RunWatchResult {
                subscription_id: self.acknowledgement().await,
                run_id: params.run_id,
                at_cursor: self.status.at_cursor.clone(),
            },
            RunSubscriptionStream::new(Quiet),
        ))
    }

    async fn run_logs(
        &self,
        _context: &ConnectionContext,
        params: RunLogsParams,
    ) -> Result<(RunLogsResult, RunLogEventStream), BackendError> {
        Ok((
            RunLogsResult {
                subscription_id: self.acknowledgement().await,
                run_id: params.run_id,
                at_cursor: self.status.at_cursor.clone(),
            },
            RunSubscriptionStream::new(Quiet),
        ))
    }

    async fn run_attach(
        &self,
        _context: &ConnectionContext,
        params: RunAttachParams,
    ) -> Result<(RunAttachResult, RunAttachEventStream), BackendError> {
        Ok((
            RunAttachResult {
                subscription_id: self.acknowledgement().await,
                run_id: params.run_id,
                execution: params.execution,
            },
            RunSubscriptionStream::new(Quiet),
        ))
    }

    async fn run_suspend(
        &self,
        _context: &ConnectionContext,
        _params: openengine_cluster_protocol::RunSuspendParams,
    ) -> Result<RunStatusResult, BackendError> {
        self.entered.notify_one();
        std::future::pending().await
    }
}

#[tokio::test]
async fn subscription_acknowledgements_and_suspend_reply_have_a_deadline() {
    let fixture = Fixture::new(false, false).await;
    let status = fixture
        .backend
        .read_recorded_status(RunStatusParams {
            run_id: fixture.id.clone(),
        })
        .await
        .unwrap();
    let controller = Arc::new(RequestController {
        status,
        status_delay: Duration::ZERO,
        acknowledge: false,
        entered: tokio::sync::Notify::new(),
    });
    let _endpoint = ServedEndpoint::start(
        &fixture.backend.paths(&fixture.id).unwrap(),
        controller.clone(),
    );
    tokio::time::timeout(LIMIT, async {
        let mut requests = Vec::new();
        for operation in ["watch", "logs", "attach", "suspend"] {
            let backend = fixture.backend.clone();
            let run_id = fixture.id.clone();
            let execution = fixture.execution.clone();
            requests.push(tokio::spawn(async move {
                match operation {
                    "watch" => backend
                        .inspect_watch(RunWatchParams {
                            run_id,
                            from_cursor: None,
                        })
                        .await
                        .map(|_| ()),
                    "logs" => backend
                        .inspect_logs(RunLogsParams {
                            run_id,
                            from_cursor: None,
                            execution: None,
                        })
                        .await
                        .map(|_| ()),
                    "attach" => backend
                        .inspect_attach(RunAttachParams { run_id, execution })
                        .await
                        .map(|_| ()),
                    _ => backend
                        .run_suspend(None, RunStatusParams { run_id })
                        .await
                        .map(|_| ()),
                }
            }));
            // Establish each connection before opening the next, including the named pipe's
            // next listening instance. Only the reply waits are concurrent.
            controller.entered.notified().await;
        }
        for request in requests {
            let result = request.await.unwrap();
            assert!(matches!(
                result,
                Err(NativeV2CliError::LocalRequestTimeout(_))
            ));
        }
    })
    .await
    .expect("subscription setup must not wait forever for acknowledgement");
}

#[tokio::test]
async fn delayed_status_is_current_and_established_streams_can_remain_quiet() {
    let fixture = Fixture::new(false, false).await;
    let status = fixture
        .backend
        .read_recorded_status(RunStatusParams {
            run_id: fixture.id.clone(),
        })
        .await
        .unwrap();
    let controller = Arc::new(RequestController {
        status: status.clone(),
        status_delay: Duration::from_secs(2),
        acknowledge: true,
        entered: tokio::sync::Notify::new(),
    });
    let _endpoint = ServedEndpoint::start(&fixture.backend.paths(&fixture.id).unwrap(), controller);
    let current = fixture
        .backend
        .status_local(RunStatusParams {
            run_id: fixture.id.clone(),
        })
        .await
        .unwrap();
    assert_eq!(current.status, status.status);
    assert!(
        current.controller.is_none(),
        "a reply within ten seconds must remain current"
    );
    let mut watch = fixture
        .backend
        .inspect_watch(RunWatchParams {
            run_id: fixture.id.clone(),
            from_cursor: None,
        })
        .await
        .unwrap();
    let mut logs = fixture
        .backend
        .inspect_logs(RunLogsParams {
            run_id: fixture.id.clone(),
            from_cursor: None,
            execution: None,
        })
        .await
        .unwrap();
    let mut attach = fixture
        .backend
        .inspect_attach(RunAttachParams {
            run_id: fixture.id.clone(),
            execution: fixture.execution.clone(),
        })
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(LIMIT, async {
            tokio::select! {
                item = watch.next() => panic!("quiet watch ended: {item:?}"),
                item = logs.next() => panic!("quiet logs ended: {item:?}"),
                item = attach.next() => panic!("quiet attach ended: {item:?}"),
            }
        })
        .await
        .is_err()
    );
}
