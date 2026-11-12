use super::*;
use crate::native_v2_cli::{BuiltinGraphTemplate, TemplateDelivery};
use crate::source_code_provider::adapters::core::github::conversations::test_support::{
    fixture, save, state,
};
use openengine_cluster_protocol::{EnumLabel, FieldName, SessionScope};
use super::routing::create_delivery_run;

struct ReviewedLane {
    delivery: Arc<NativeV2DeliveryAdapter>,
    workspace: PathBuf,
    worker_visits: AtomicUsize,
    reviewers: AtomicUsize,
    respond: bool,
    ci_repair_program: Option<PathBuf>,
    forge: Arc<FakeGitHub>,
}
#[async_trait]
impl SessionFactory for ReviewedLane {
    async fn open(
        &self,
        invocation: &NodeInvocation,
        environment: &ResolvedEnvironment,
    ) -> Result<Arc<dyn NodeSession>, NodeRunnerError> {
        SessionFactory::open(self.delivery.as_ref(), invocation, environment)
            .await
            .or_else(|error| {
                if matches!(invocation.binding, NodeRuntimeBinding::Agent { .. }) {
                    Ok(Arc::new(super::routing::RepairSession) as Arc<dyn NodeSession>)
                } else {
                    Err(error)
                }
            })
    }
}
#[async_trait]
impl NodeDriver for ReviewedLane {
    async fn run(
        &self,
        invocation: DriverInvocation,
        control: DriverControl,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        if matches!(
            invocation.node.binding,
            NodeRuntimeBinding::GitDelivery { .. }
        ) {
            return NodeDriver::run(self.delivery.as_ref(), invocation, control).await;
        }
        assert!(
            control.delivery_state().is_none(),
            "receipts stay outside agent state"
        );
        assert!(
            invocation
                .environment
                .iter()
                .all(|(key, _)| key.as_str() != "GH_TOKEN")
        );
        match invocation.node.reference.node.as_str() {
            "worker" => {
                let visit = self.worker_visits.fetch_add(1, Ordering::SeqCst);
                let responses = if visit == 2
                    && let Some(program) = &self.ci_repair_program
                {
                    assert_eq!(invocation.node.input["phase"]["after"], "ci_failed");
                    assert!(
                        invocation.node.input["deliveryFeedback"]
                            .as_str()
                            .unwrap()
                            .contains("Required CI checks failed")
                    );
                    let mut data = state(program);
                    assert_eq!(data["threads"][0]["resolved"], true);
                    data["threads"][0]["comments"][0]["body"] =
                        json!("Finding edited during CI repair.");
                    data["threads"][0]["comments"][0]["updated_at"] = json!("two");
                    save(program, &data);
                    // Simulate controller recovery: all adapter-local receipt maps are lost.
                    // The next delivery must recover them from host execution history even
                    // though this CI repair returns no review responses.
                    *self
                        .forge
                        .feedback_authority
                        .as_ref()
                        .unwrap()
                        .0
                        .lock()
                        .await = GhCliDeliveryAuthority::new(GhCliAuthorityConfig {
                        gh_program: program.clone(),
                        api_deadline: Duration::from_secs(10),
                        ..GhCliAuthorityConfig::hosted(self.workspace.parent().unwrap().into())
                    });
                    fs::write(self.workspace.join("ci-fix.txt"), "CI repaired\n").unwrap();
                    Vec::new()
                } else if visit == 0 || !self.respond {
                    Vec::new()
                } else {
                    assert_eq!(invocation.node.input["phase"]["after"], "repair_required");
                    let feedback = invocation.node.input["deliveryFeedback"].as_str().unwrap();
                    let item: Value = serde_json::from_str(
                        feedback
                            .split("--- feedback item ---")
                            .nth(1)
                            .unwrap()
                            .trim(),
                    )
                    .unwrap();
                    fs::write(self.workspace.join("response.txt"), "empty input guarded\n")
                        .unwrap();
                    vec![
                        json!({"threadId":item["threadId"],"version":item["version"],"disposition":"addressed","response":"Guarded empty input; regression verified."}),
                    ]
                };
                Ok(WorkerOutcome::Verified {
                    output: json!({"reviewResponses":responses}),
                    artifacts: Vec::new(),
                })
            }
            node @ ("acceptance" | "code") => {
                self.reviewers.fetch_add(1, Ordering::SeqCst);
                let responses = invocation.node.input["reviewResponses"].as_array().unwrap();
                if !responses.is_empty() {
                    assert_eq!(responses[0]["threadId"], "THREAD_1");
                    assert_eq!(
                        responses[0]["response"],
                        "Guarded empty input; regression verified."
                    );
                    assert_eq!(
                        fs::read_to_string(self.workspace.join("response.txt")).unwrap(),
                        "empty input guarded\n"
                    );
                }
                Ok(WorkerOutcome::Verifier {
                    output: if node == "acceptance" {
                        json!({"title":"fix: guard empty input","description":"Guard empty input and respond to the review finding."})
                    } else {
                        Value::Null
                    },
                    signals: BTreeMap::from([(
                        FieldName::new("verdict").unwrap(),
                        EnumLabel::new("accepted").unwrap(),
                    )]),
                    diagnostic: json!({"message":"Independently verified the repair and conversation disposition."}),
                    artifacts: Vec::new(),
                })
            }
            _ => Err(NodeRunnerError::Driver),
        }
    }
}

#[tokio::test]
async fn app_conversation_repairs_through_both_existing_reviewers_then_resolves_and_merges() {
    run_conversation_loop("Bot", true, None).await;
}

#[tokio::test]
async fn unchanged_unresolved_conversations_block_delivery_without_another_repair() {
    run_conversation_loop("Bot", false, None).await;
}

#[tokio::test]
async fn unauthorized_blocking_conversations_never_trigger_repair_or_resolution() {
    run_conversation_loop("User", false, None).await;
}

#[tokio::test]
async fn mixed_authorized_and_unauthorized_participants_require_a_maintainer_before_repair() {
    run_conversation_loop("Bot", false, Some("mixed_participants")).await;
}

#[tokio::test]
async fn lost_reply_acknowledgement_reconciles_without_another_worker_or_review_pass() {
    run_conversation_loop("Bot", true, Some("lose_reply_response")).await;
}

#[tokio::test]
async fn interrupted_resolution_of_an_edited_thread_requires_another_repair_and_both_reviews() {
    run_conversation_loop("Bot", true, Some("edit_after_resolve")).await;
}

#[tokio::test]
async fn untrusted_reply_during_interrupted_resolution_blocks_merge_without_another_repair() {
    run_conversation_loop("Bot", true, Some("unauthorized_reply_after_resolve")).await;
}

#[tokio::test]
async fn confirmed_conversation_changed_during_later_ci_repair_is_reassessed_after_empty_responses()
{
    run_conversation_loop("Bot", true, Some("edit_during_ci_repair")).await;
}

async fn run_conversation_loop(kind: &str, respond: bool, race: Option<&str>) {
    let repo = TempRepo::delivery();
    let receipt = SourceReviewReceipt {
        repository: "acme/project".into(),
        review_id: "17".into(),
        target_branch: "main".into(),
        head_branch: "pending".into(),
        head_revision: repo.base.clone(),
    };
    let program = fixture(repo.root.path(), &receipt, kind);
    let mut data = state(&program);
    data["move_positions"] = json!(true);
    let ci_repair = race == Some("edit_during_ci_repair");
    let mixed = race == Some("mixed_participants");
    if mixed {
        let mut reply = data["threads"][0]["comments"][0].clone();
        reply["id"] = json!(42);
        reply["in_reply_to_id"] = json!(41);
        reply["user"] = json!({"id":9,"login":"reader","type":"User"});
        reply["body"] = json!("An unauthorized request.");
        data["threads"][0]["comments"]
            .as_array_mut()
            .unwrap()
            .push(reply);
    } else if race == Some("lose_reply_response") {
        data["lose_reply_response"] = json!(true);
    } else if let Some(race) = race.filter(|_| !ci_repair) {
        data["race"] = json!(race);
        data["fail_confirmation_reads"] = json!(1);
    }
    save(&program, &data);
    let forge = GhCliDeliveryAuthority::new(GhCliAuthorityConfig {
        gh_program: program.clone(),
        api_deadline: Duration::from_secs(10),
        ..GhCliAuthorityConfig::hosted(repo.root.path().into())
    });
    let mut authority = FakeGitHub::new(
        repo.remote.clone(),
        if ci_repair {
            Script::ConversationThenCiRepair
        } else {
            Script::NoCi
        },
    );
    authority.feedback_authority = Some((tokio::sync::Mutex::new(forge), program.clone()));
    let authority = Arc::new(authority);
    let graph = BuiltinGraphTemplate::SoftwareChange
        .materialize(TemplateDelivery::Merge)
        .unwrap();
    let mut nodes = BTreeMap::new();
    for name in ["worker", "acceptance", "code"] {
        nodes.insert(
            NodeName::new(name).unwrap(),
            NodeRuntimeBinding::Agent {
                lane: None,
                model: native_v2_contract::ModelId::new("fixture").unwrap(),
                effort: None,
                harness_config: None,
                context_window: None,
                max_tokens: None,
                session_scope: SessionScope::Execution,
                connections: DeclaredConnections::empty(),
            },
        );
    }
    nodes.insert(
        NodeName::new("deliver").unwrap(),
        NodeRuntimeBinding::GitDelivery {
            connections: DeclaredConnections::single(
                "github",
                DeclaredEnvironment::new([EnvironmentVariableName::new("GH_TOKEN").unwrap()])
                    .unwrap(),
            )
            .unwrap(),
            pull_request_feedback: PullRequestFeedback::Consider,
        },
    );
    let admitted = NativeV2Admission
        .admit(RunSubmission {
            environment: None,
            title: RunTitle::new("Verified conversation repair").unwrap(),
            graph,
            initial_input: json!({"task":"Guard empty input"}),
            runtime: crate::native_v2_candidate::test_support::codex_runtime(
                run_size("medium"),
                nodes,
            ),
            source: source_for_revision(&repo.base),
            submission_key: IdempotencyKey::new("conversation-repair").unwrap(),
        })
        .await
        .unwrap();
    let delivery = Arc::new(NativeV2DeliveryAdapter::new(
        NativeV2DeliveryConfig {
            delivery_run_id: RunId::new("conversation-repair"),
            adopt_existing_delivery: false,
            local_branch: None,
            git_identity: None,
            commit_identity: None,
            commit_co_author: None,
            workspace: repo.workspace.clone(),
            git_program: "/usr/bin/git".into(),
            target: target(&repo),
            poll: DeliveryPollPolicy::new(3, Duration::ZERO).unwrap(),
        },
        authority.clone(),
    ));
    let lane = Arc::new(ReviewedLane {
        delivery,
        workspace: repo.workspace.clone(),
        worker_visits: AtomicUsize::new(0),
        reviewers: AtomicUsize::new(0),
        respond,
        ci_repair_program: ci_repair.then(|| program.clone()),
        forge: authority.clone(),
    });
    let runner = Arc::new(NativeNodeRunner::new(&admitted, lane.clone(), lane.clone()).unwrap());
    let (run_id, ledger, environments) = create_delivery_run(admitted).await;
    let terminal = tokio::time::timeout(
        Duration::from_secs(30),
        NativeV2Supervisor::new(run_id.clone(), ledger.clone(), runner, environments).drive(),
    )
    .await
    .expect("conversation repair graph must finish")
    .unwrap();
    let edited = race == Some("edit_after_resolve") || ci_repair;
    let untrusted_reply = race == Some("unauthorized_reply_after_resolve");
    let expected_merge = respond && !untrusted_reply;
    let expected_repairs =
        usize::from(kind == "Bot" && !mixed) + usize::from(edited) + usize::from(ci_repair);
    assert_eq!(
        matches!(terminal, TerminalResult::Succeeded { .. }),
        expected_merge,
        "{terminal:?}"
    );
    assert_eq!(
        lane.worker_visits.load(Ordering::SeqCst),
        1 + expected_repairs
    );
    assert_eq!(
        lane.reviewers.load(Ordering::SeqCst),
        2 * (1 + expected_repairs)
    );
    assert_eq!(
        authority.merge_requests.load(Ordering::SeqCst),
        usize::from(expected_merge)
    );
    let data = state(&program);
    assert_eq!(data["threads"][0]["resolved"], expected_merge);
    assert_eq!(
        data["threads"][0]["comments"].as_array().unwrap().len(),
        1 + usize::from(respond) + usize::from(edited || untrusted_reply || mixed)
    );
    if !respond || untrusted_reply {
        assert!(
            format!("{terminal:?}").contains(if kind == "User" || untrusted_reply || mixed {
                "automated repair and resolution are not authorized"
            } else {
                "previously forwarded feedback is not resolution"
            }),
            "{terminal:?}"
        );
        if !respond {
            assert!(!data["calls"].to_string().contains("resolveReviewThread"));
        }
    }
    let stored = ledger.get(&run_id).await.unwrap().unwrap();
    let restored_history = crate::native_v2_supervisor::durable_history(&stored.snapshot).unwrap();
    let seed = serde_json::to_vec(&restored_history).unwrap();
    let restored_history: Vec<crate::full_v1_reducer::DurableExecution> =
        serde_json::from_slice(&seed).unwrap();
    assert!(
        restored_history
            .iter()
            .filter(|execution| execution.occurrence.node.as_str() == "deliver")
            .all(|execution| execution.delivery_state.is_some())
    );

    assert_eq!(
        stored
            .snapshot
            .executions
            .values()
            .filter(|execution| execution.reference.node.as_str() == "deliver")
            .count(),
        1 + expected_repairs
    );
}
