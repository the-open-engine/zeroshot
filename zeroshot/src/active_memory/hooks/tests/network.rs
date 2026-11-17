use super::*;
use openengine_cluster_protocol::{
    NetworkSteerParams, NodeName, RunId, RunSteerResult, SteeringMode, SteeringNetwork,
};
use crate::native_v2_contract::{ExecutionId, NodeInstanceId};
use crate::v2_run_ledger::{CreateRun, RunLedger};

fn name(value: &str) -> NodeName {
    NodeName::new(value).assert_value()
}
fn networks() -> BTreeMap<NodeName, SteeringNetwork> {
    [
        ("alpha", vec!["worker", "reader"], vec!["worker"]),
        ("beta", vec!["worker", "reader", "future"], vec!["worker"]),
        (
            "closed",
            vec!["worker", "reader", "blocked"],
            vec!["blocked"],
        ),
    ]
    .into_iter()
    .map(|(id, members, publishers)| {
        (
            name(id),
            SteeringNetwork {
                members: members.into_iter().map(name).collect(),
                publishers: publishers.into_iter().map(name).collect(),
            },
        )
    })
    .collect()
}
fn turn(node: &str, instance: u64, execution: u64, home: u64) -> DriverInvocation {
    let mut invocation = invocation();
    invocation.node.reference.node = name(node);
    invocation.node.reference.node_instance = NodeInstanceId::new(instance).assert_value();
    invocation.node.reference.execution = ExecutionId::new(execution).assert_value();
    invocation.provider_home_slot = NodeInstanceId::new(home).assert_value();
    invocation
}
struct NetworkFixture {
    hooks: Arc<ActiveMemoryHooks>,
    ledger: Arc<dyn RunLedger>,
    run: RunId,
    home: tempfile::TempDir,
    sender: DriverInvocation,
    capability: std::path::PathBuf,
}
impl NetworkFixture {
    async fn new() -> Self {
        let (ledger, run) = steer_record().await;
        let hooks = Arc::new(ActiveMemoryHooks::empty().assert_value());
        hooks.configure_networks(&networks());
        hooks.bind_ledger(run.clone(), ledger.clone());
        let home = tempfile::tempdir().assert_value();
        let sender = turn("worker", 1, 2, 1);
        let capability = hooks
            .capability_file(&sender, home.path(), home.path())
            .assert_value()
            .assert_value();
        Self {
            hooks,
            ledger,
            run,
            home,
            sender,
            capability,
        }
    }
    fn params(&self, text: &str, mode: SteeringMode, selected: &[&str]) -> NetworkSteerParams {
        NetworkSteerParams {
            run_id: self.run.clone(),
            text: text.into(),
            mode,
            networks: selected.iter().map(|n| name(n)).collect(),
        }
    }
    async fn publish(&self, params: NetworkSteerParams) -> std::io::Result<RunSteerResult> {
        let capability = self.capability.clone();
        tokio::task::spawn_blocking(move || publish_network(&capability, params))
            .await
            .assert_value()
    }
    async fn prepare(&self, invocation: &mut DriverInvocation) -> String {
        self.hooks
            .prepare_steers(invocation, false)
            .await
            .assert_value();
        invocation.host_context.clone().unwrap_or_default()
    }
    async fn deliver(&self, invocation: &DriverInvocation) -> (HookQuery, HookDelivery) {
        self.hooks
            .tool_delivery(invocation, &json!({"tool_name":"Read"}), self.home.path())
            .await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn network_publication_checks_every_mandate_and_stamps_host_identity() {
    let fixture = NetworkFixture::new().await;
    for selected in [&["alpha", "closed"][..], &["unknown"][..], &[][..]] {
        let error = fixture
            .publish(fixture.params("denied", SteeringMode::Once, selected))
            .await
            .unwrap_err();
        assert!(!error.to_string().is_empty());
    }
    let mut foreign = fixture.params("foreign", SteeringMode::Once, &["alpha"]);
    foreign.run_id = RunId::new("another-run");
    assert!(
        fixture
            .publish(foreign)
            .await
            .unwrap_err()
            .to_string()
            .contains("another run")
    );
    let member = turn("reader", 9, 9, 9);
    let path = fixture
        .hooks
        .capability_file(
            &member,
            fixture.home.path(),
            &fixture.home.path().join("member"),
        )
        .assert_value()
        .assert_value();
    let params = fixture.params("member", SteeringMode::Once, &["alpha"]);
    assert!(
        tokio::task::spawn_blocking(move || publish_network(&path, params))
            .await
            .assert_value()
            .unwrap_err()
            .to_string()
            .contains("not authorized")
    );
    assert!(
        crate::v2_run_ledger::steering::status(fixture.ledger.as_ref(), &fixture.run)
            .await
            .assert_value()
            .steers
            .is_empty()
    );
    let published = fixture
        .publish(fixture.params("peer-note", SteeringMode::Once, &["beta", "alpha", "alpha"]))
        .await
        .assert_value();
    let metadata = published.steer.network.assert_value();
    assert_eq!(metadata.networks, vec![name("alpha"), name("beta")]);
    assert_eq!(metadata.origin.node, fixture.sender.node.reference.node);
    assert_eq!(metadata.origin.node_instance, 1);
    assert_eq!(
        metadata.origin.execution,
        fixture
            .sender
            .node
            .reference
            .public_execution()
            .assert_value()
    );
    assert_eq!(published.steer.mode, SteeringMode::Once);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn network_roster_environment_and_capability_follow_publisher_authority() {
    let fixture = NetworkFixture::new().await;
    let mut sender = fixture.sender.clone();
    let context = fixture.prepare(&mut sender).await;
    assert!(context.contains("alpha: members reader, worker"));
    assert!(context.contains("beta: members future, reader, worker"));
    assert!(!context.contains("closed: members"));
    let executable = super::super::native::hook_executable().assert_value();
    let command = format!(
        "{} steer {} --network NAME --mode once --text-stdin",
        super::super::native::shell_quote(&executable),
        super::super::native::shell_quote(Path::new(fixture.run.as_str()))
    );
    assert!(context.contains(&command));
    assert!(context.contains("Network messages default to once"));
    let mut outsider = turn("outsider", 9, 9, 9);
    assert!(fixture.prepare(&mut outsider).await.is_empty());
    let mut environment =
        BTreeMap::from([("ZEROSHOT_STEERING_CAPABILITY".into(), "untrusted".into())]);
    fixture.hooks.configure_network_environment(
        &mut environment,
        &outsider,
        Some(&fixture.capability),
    );
    assert!(!environment.contains_key("ZEROSHOT_STEERING_CAPABILITY"));
    fixture.hooks.configure_network_environment(
        &mut environment,
        &sender,
        Some(&fixture.capability),
    );
    assert_eq!(
        environment["ZEROSHOT_STEERING_CAPABILITY"],
        fixture.capability.to_string_lossy()
    );
    sender.auxiliary = true;
    sender.host_context = None;
    assert!(!fixture.hooks.publishing_authorized(&sender));
    assert!(
        fixture
            .hooks
            .prepare_steers(&mut sender, false)
            .await
            .is_none()
    );
    assert!(
        fixture
            .hooks
            .capability_file(&sender, fixture.home.path(), fixture.home.path())
            .assert_value()
            .is_none()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn network_overlap_queues_future_instances_without_echoing_the_origin() {
    let fixture = NetworkFixture::new().await;
    fixture
        .publish(fixture.params("overlap-note", SteeringMode::Once, &["alpha", "beta"]))
        .await
        .assert_value();
    for (node, instance) in [("reader", 7), ("worker", 2), ("future", 8)] {
        let mut recipient = turn(node, instance, instance + 10, instance);
        let context = fixture.prepare(&mut recipient).await;
        assert_eq!(context.matches("overlap-note").count(), 1);
        assert!(context.contains("<network-steer>"));
        assert!(!context.contains("<steer>"));
        fixture
            .hooks
            .finish_steers(recipient.node.reference.execution.get(), true)
            .await;
    }
    for (node, instance) in [("worker", 1), ("outsider", 9)] {
        let mut excluded = turn(node, instance, instance + 30, instance + 30);
        assert!(
            !fixture
                .prepare(&mut excluded)
                .await
                .contains("overlap-note")
        );
    }
    let status = crate::v2_run_ledger::steering::status(fixture.ledger.as_ref(), &fixture.run)
        .await
        .assert_value();
    assert_eq!(status.steers.len(), 1);
    assert_eq!(status.steers[0].received_executions, 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn network_once_survives_fresh_conversations_while_standing_is_offered_again() {
    let fixture = NetworkFixture::new().await;
    for (text, mode) in [
        ("once-note", SteeringMode::Once),
        ("standing-note", SteeringMode::Standing),
    ] {
        fixture
            .publish(fixture.params(text, mode, &["alpha"]))
            .await
            .assert_value();
    }
    let mut first = turn("reader", 7, 7, 7);
    let context = fixture.prepare(&mut first).await;
    assert!(context.contains("once-note") && context.contains("standing-note"));
    fixture.hooks.finish_steers(7, true).await;
    let mut same = turn("reader", 7, 8, 7);
    assert!(fixture.prepare(&mut same).await.is_empty());
    let mut fresh = turn("reader", 7, 9, 9);
    let context = fixture.prepare(&mut fresh).await;
    assert!(!context.contains("once-note"));
    assert!(context.contains("standing-note"));
    let mut copy = turn("reader", 8, 10, 10);
    let context = fixture.prepare(&mut copy).await;
    assert!(context.contains("once-note") && context.contains("standing-note"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn network_lost_ack_reservations_release_and_failed_start_does_not_consume_once() {
    let fixture = NetworkFixture::new().await;
    let mut reader = turn("reader", 7, 7, 7);
    assert!(fixture.prepare(&mut reader).await.is_empty());
    fixture
        .publish(fixture.params("lost-ack-note", SteeringMode::Once, &["alpha"]))
        .await
        .assert_value();
    let (query, first) = fixture.deliver(&reader).await;
    assert_eq!(first.steers.len(), 1);
    assert!(fixture.deliver(&reader).await.1.steers.is_empty());
    fixture.hooks.discard_delivery(&query, &first).await;
    let (query, retry) = fixture.deliver(&reader).await;
    assert_eq!(retry.steers, first.steers);
    fixture.hooks.acknowledge(&query, &retry, true).await;
    assert!(fixture.deliver(&reader).await.1.steers.is_empty());
    fixture
        .publish(fixture.params("no-carrier-note", SteeringMode::Once, &["alpha"]))
        .await
        .assert_value();
    assert_eq!(fixture.deliver(&reader).await.1.steers.len(), 1);
    fixture.hooks.finish_steers(7, true).await;
    let mut fresh = turn("reader", 7, 8, 8);
    let context = fixture.prepare(&mut fresh).await;
    assert!(!context.contains("lost-ack-note"));
    assert!(context.contains("no-carrier-note"));
    fixture.hooks.finish_steers(8, false).await;
    let mut failed_start = turn("reader", 7, 9, 9);
    assert!(
        fixture
            .prepare(&mut failed_start)
            .await
            .contains("no-carrier-note")
    );
}

async fn successor(fixture: &NetworkFixture) -> (ActiveMemoryHooks, RunId) {
    let next = RunId::new("network-successor");
    let stored = fixture
        .ledger
        .get(&fixture.run)
        .await
        .assert_value()
        .assert_value();
    fixture
        .ledger
        .create_or_get(CreateRun {
            run_id: next.clone(),
            submission_key: openengine_cluster_protocol::IdempotencyKey::new("network-successor")
                .assert_value(),
            submission_digest: stored.submission_digest,
            admitted: stored.admitted,
        })
        .await
        .assert_value();
    crate::v2_run_ledger::vectors::inherit(
        fixture.ledger.as_ref(),
        fixture.ledger.as_ref(),
        &fixture.run,
        &next,
    )
    .await
    .assert_value();
    let hooks = ActiveMemoryHooks::empty().assert_value();
    hooks.configure_networks(&networks());
    hooks.bind_ledger(next.clone(), fixture.ledger.clone());
    (hooks, next)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn network_successor_keeps_once_receipts_and_always_excludes_the_origin_instance() {
    let fixture = NetworkFixture::new().await;
    for (text, mode) in [
        ("resume-once", SteeringMode::Once),
        ("resume-standing", SteeringMode::Standing),
    ] {
        fixture
            .publish(fixture.params(text, mode, &["alpha", "beta"]))
            .await
            .assert_value();
    }
    let mut reader = turn("reader", 7, 7, 7);
    fixture.prepare(&mut reader).await;
    fixture.hooks.finish_steers(7, true).await;
    let saved = fixture.hooks.capture_steers();
    let (restored, next) = successor(&fixture).await;
    restored.restore_steers(7, saved[&reader.node.reference.node_instance].clone());
    reader.node.reference.run_id = next.clone();
    reader.host_context = None;
    restored
        .prepare_steers(&mut reader, true)
        .await
        .assert_value();
    assert!(reader.host_context.is_none());
    let fallback = ActiveMemoryHooks::empty().assert_value();
    fallback.configure_networks(&networks());
    fallback.bind_ledger(next.clone(), fixture.ledger.clone());
    reader.provider_home_slot = NodeInstanceId::new(8).assert_value();
    fallback
        .prepare_steers(&mut reader, false)
        .await
        .assert_value();
    let context = reader.host_context.as_ref().assert_value();
    assert!(!context.contains("resume-once"));
    assert!(context.contains("resume-standing"));
    let mut origin = turn("worker", 1, 20, 20);
    origin.node.reference.run_id = next;
    fallback
        .prepare_steers(&mut origin, false)
        .await
        .assert_value();
    assert!(
        !origin
            .host_context
            .as_ref()
            .assert_value()
            .contains("resume-once")
    );
    assert!(
        !origin
            .host_context
            .as_ref()
            .assert_value()
            .contains("resume-standing")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn network_replaced_and_finished_capabilities_fail_explicitly() {
    let fixture = NetworkFixture::new().await;
    let old = std::fs::read(&fixture.capability).assert_value();
    let replacement = fixture
        .hooks
        .capability_file(&fixture.sender, fixture.home.path(), fixture.home.path())
        .assert_value()
        .assert_value();
    let stale = fixture.home.path().join("stale.json");
    std::fs::write(&stale, old).assert_value();
    let params = fixture.params("stale-note", SteeringMode::Once, &["alpha"]);
    assert!(
        tokio::task::spawn_blocking(move || publish_network(&stale, params))
            .await
            .assert_value()
            .unwrap_err()
            .to_string()
            .contains("retired")
    );
    fixture.hooks.finish_steers(2, false).await;
    let params = fixture.params("finished-note", SteeringMode::Once, &["alpha"]);
    assert!(
        tokio::task::spawn_blocking(move || publish_network(&replacement, params))
            .await
            .assert_value()
            .unwrap_err()
            .to_string()
            .contains("retired")
    );
    assert!(
        crate::v2_run_ledger::steering::status(fixture.ledger.as_ref(), &fixture.run)
            .await
            .assert_value()
            .steers
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn network_native_broker_reoffers_after_missing_ack_and_accepts_complete_maximum_text() {
    let fixture = NetworkFixture::new().await;
    let mut reader = turn("reader", 7, 7, 7);
    fixture.prepare(&mut reader).await;
    let maximum = "x".repeat(openengine_cluster_protocol::MAX_NODE_INSTRUCTIONS_BYTES);
    fixture
        .publish(fixture.params(&maximum, SteeringMode::Once, &["alpha"]))
        .await
        .assert_value();
    let capability = fixture
        .hooks
        .broker
        .capability(&reader, fixture.home.path())
        .assert_value();
    tokio::task::spawn_blocking(move || {
        let (first, stream) =
            callback_response(&capability, &capability.token, json!({"tool_name":"Read"}));
        assert!(first.contains(&maximum));
        assert!(first.len() <= super::super::native::MAX_CONTEXT_BYTES);
        drop(stream);
        let second = callback(&capability, &capability.token, json!({"tool_name":"Read"}));
        assert_eq!(second, first);
        assert!(callback(&capability, &capability.token, json!({"tool_name":"Read"})).is_empty());
    })
    .await
    .assert_value();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn network_transient_callback_reoffers_only_standing_until_a_prompt_succeeds() {
    let fixture = NetworkFixture::new().await;
    let mut reader = turn("reader", 7, 7, 7);
    assert!(fixture.prepare(&mut reader).await.is_empty());
    for (text, mode) in [
        ("once-transient", SteeringMode::Once),
        ("standing-transient", SteeringMode::Standing),
    ] {
        fixture
            .publish(fixture.params(text, mode, &["alpha", "beta"]))
            .await
            .assert_value();
    }
    let (query, delivery) = fixture.deliver(&reader).await;
    assert_eq!(delivery.steers.len(), 2);
    fixture.hooks.acknowledge(&query, &delivery, false).await;
    assert!(fixture.deliver(&reader).await.1.steers.is_empty());
    let instance = reader.node.reference.node_instance;
    assert_eq!(fixture.hooks.capture_steers()[&instance].receipts.len(), 1);
    fixture.hooks.finish_steers(7, true).await;
    for (execution, success) in [(8, false), (9, true)] {
        let mut revisit = turn("reader", 7, execution, 7);
        let context = fixture.prepare(&mut revisit).await;
        assert!(!context.contains("once-transient"));
        assert_eq!(context.matches("standing-transient").count(), 1);
        assert_eq!(fixture.hooks.capture_steers()[&instance].receipts.len(), 1);
        fixture.hooks.finish_steers(execution, success).await;
    }
    assert_eq!(fixture.hooks.capture_steers()[&instance].receipts.len(), 2);
    let mut settled = turn("reader", 7, 10, 7);
    assert!(fixture.prepare(&mut settled).await.is_empty());
    let mut sender = fixture.sender.clone();
    let context = fixture.prepare(&mut sender).await;
    assert!(!context.contains("once-transient") && !context.contains("standing-transient"));
    let status = crate::v2_run_ledger::steering::status(fixture.ledger.as_ref(), &fixture.run)
        .await
        .assert_value();
    assert!(
        status
            .steers
            .iter()
            .all(|steer| steer.received_executions == 1)
    );
}
