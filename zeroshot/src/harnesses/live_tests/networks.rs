//! Real native tool publication with provider credentials held only in memory/environment.
use super::*;
use openengine_cluster_protocol::{RunVectorContent, SteeringNetwork};
use crate::native_v2_runner::{NativeNodeRunner, NodeRunRequest};
use crate::v2_run_ledger::RunLedger;

struct LiveNetwork {
    settings: LiveSettings,
    bindings: BTreeMap<NodeName, NodeRuntimeBinding>,
    run: RunId,
    ledger: Arc<dyn RunLedger>,
    runner: NativeNodeRunner,
    _directory: TestDirectory,
}

const INSTRUCTIONS: &str = "Exercise the peer network by following the task in your input.";

impl LiveNetwork {
    async fn new(names: &[&str], publisher: &str) -> Self {
        let mut settings = LiveSettings::read();
        let registry = HarnessRegistry::local();
        let directory = TestDirectory::new("live-networks");
        let user = directory.child("user");
        std::fs::create_dir(&user).assert_value();
        for name in ["HOME", "CLAUDE_CONFIG_DIR", "CODEX_HOME"] {
            settings
                .environment
                .insert(name.into(), user.display().to_string());
        }
        let run = RunId::new(format!("network-live-{}", uuid::Uuid::now_v7()));
        let bindings = names
            .iter()
            .map(|name| {
                (
                    NodeName::new(*name).assert_value(),
                    settings.binding(&registry, SessionScope::Execution),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let mut children = names
            .iter()
            .map(|name| {
                let mut node = executable(name, INSTRUCTIONS, false);
                node["input"] = json!({
                    "kind":"record",
                    "fields":{"task":{"type":{"kind":"string"},"required":false}}
                });
                node["output"] = json!({"kind":"array","items":{"kind":"string"}});
                node
            })
            .collect::<Vec<_>>();
        children.push(success_node());
        let mut graph = full_graph(children);
        let members = names
            .iter()
            .filter(|name| **name != "outsider")
            .map(|name| NodeName::new(*name).assert_value())
            .collect::<BTreeSet<_>>();
        let definition = SteeringNetwork {
            members,
            publishers: BTreeSet::from([NodeName::new(publisher).assert_value()]),
        };
        graph.networks = ["alpha", "beta"]
            .into_iter()
            .map(|name| (NodeName::new(name).assert_value(), definition.clone()))
            .collect();
        let admitted = NativeV2Admission
            .admit(live_submission(
                &settings,
                &bindings,
                graph,
                ("Live graph network communication", run.as_str()),
            ))
            .await
            .unwrap_or_else(|error| panic!("live network admission failed: {error}"));
        let ledger = create_live_record(&run, &admitted).await;
        let hooks = Arc::new(ActiveMemoryHooks::empty().assert_value());
        hooks.bind_ledger(run.clone(), ledger.clone());
        hooks.configure_networks(&admitted.graph.networks);
        let runner = live_runner(&settings, &admitted, directory.child("runtime"), hooks);
        Self {
            settings,
            bindings,
            run,
            ledger,
            runner,
            _directory: directory,
        }
    }

    fn request(&self, node: &str, identity: (u64, u64), instruction: &str) -> NodeRunRequest {
        let binding = self.bindings[&NodeName::new(node).assert_value()].clone();
        NodeRequestFixture {
            run_id: self.run.as_str(),
            node,
            node_instance: identity.0,
            execution: identity.1,
            worker: &format!("agent.{node}@1"),
            instructions: INSTRUCTIONS,
            input: json!({"task":instruction}),
            environment: self.settings.connection_values(&binding),
            binding,
        }
        .into_request()
    }

    async fn turn(&self, node: &str, identity: (u64, u64), instruction: &str) -> Vec<String> {
        let mut handle = self
            .runner
            .start(self.request(node, identity, instruction))
            .await
            .unwrap_or_else(|error| panic!("live network start failed: {error}"));
        messages(completion(&mut handle).await)
    }

    async fn entries(&self) -> Vec<openengine_cluster_protocol::RunVector> {
        crate::v2_run_ledger::steering::all(self.ledger.as_ref(), &self.run)
            .await
            .assert_value()
    }
}

fn messages(completion: crate::native_v2_contract::NodeCompletion) -> Vec<String> {
    let WorkerOutcome::Verified { output, .. } = completion.outcome else {
        panic!("live network node failed: {:?}", completion.outcome);
    };
    let mut result: Vec<String> = serde_json::from_value(output).assert_value();
    result.sort();
    result
}

const RECEIVE: &str = concat!(
    "First make one NEW native shell call `printf peer-network-callback-proof`. ",
    "Return an array containing exactly the text of each peer message actually delivered in a ",
    "<network-steer> block of your host context or native tool callback, once per distinct text. ",
    "Do not include attribution, vector IDs, or unrelated guidance. If none arrived, return []. ",
    "Do not edit files, call other tools, invent messages, or use Git. Follow the output contract."
);

fn publish_command(once: &str, standing: &str) -> String {
    let cli = crate::active_memory::hooks::native::shell_quote(&PathBuf::from(required(
        "ZEROSHOT_LIVE_CLI_BINARY",
    )));
    format!(
        "{cli} steer \"$ZEROSHOT_RUN_ID\" --network alpha --network beta \"{once}\" && \
        {cli} steer \"$ZEROSHOT_RUN_ID\" --network beta --network alpha --mode standing \"{standing}\""
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires ZEROSHOT_LIVE_* settings, actual native harness, API credentials and CLI binary"]
async fn native_network_messages_once_standing_fresh_conversations_and_denial() {
    let fixture = LiveNetwork::new(&["sender", "receiver", "outsider"], "sender").await;
    let once = format!("one-off-{}", uuid::Uuid::now_v7());
    let standing = format!("standing-{}", uuid::Uuid::now_v7());
    let instruction = format!(
        "Your FIRST action must be ONE native shell call running this exact command: `{}`. \
         Check both commands succeeded, then return []. You have no incoming peer messages. \
         Do not edit files or use any other tools. Follow the output contract.",
        publish_command(&once, &standing),
    );
    assert!(
        fixture
            .turn("sender", (1, 1), &instruction)
            .await
            .is_empty()
    );
    assert!(fixture.turn("sender", (1, 2), RECEIVE).await.is_empty());
    let mut expected = vec![once.clone(), standing.clone()];
    expected.sort();
    assert_eq!(fixture.turn("receiver", (2, 3), RECEIVE).await, expected);
    assert_eq!(
        fixture.turn("receiver", (2, 4), RECEIVE).await,
        vec![standing]
    );
    let cli = crate::active_memory::hooks::native::shell_quote(&PathBuf::from(required(
        "ZEROSHOT_LIVE_CLI_BINARY",
    )));
    let outsider = format!(
        "First run a native shell call `{cli} steer \"$ZEROSHOT_RUN_ID\" --network alpha forbidden-message`. \
         Confirm it fails with no publishing mandate, then return []. There are no incoming messages. \
         Do not edit files or use other tools. Follow the output contract."
    );
    assert!(fixture.turn("outsider", (3, 5), &outsider).await.is_empty());
    fixture.runner.close_run(&fixture.run).await;
    let entries = fixture.entries().await;
    assert_eq!(
        entries
            .iter()
            .filter(|v| matches!(v.content, RunVectorContent::Steer { .. }))
            .count(),
        2
    );
    let receipts = entries
        .iter()
        .filter_map(|v| match &v.content {
            RunVectorContent::SteerReceipt { node_instance, .. } => Some(*node_instance),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(receipts, vec![2, 2, 2]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires ZEROSHOT_LIVE_* settings, actual native harness, API credentials and CLI binary"]
async fn native_parallel_network_peers_and_future_recipient() {
    let fixture = LiveNetwork::new(&["peer", "tail"], "peer").await;
    let tag = format!("parallel-{}", uuid::Uuid::now_v7());
    let barrier = fixture._directory.child("barrier");
    std::fs::create_dir(&barrier).assert_value();
    let barrier = crate::active_memory::hooks::native::shell_quote(&barrier);
    let once = format!("{tag}-once-$ZEROSHOT_NODE_INSTANCE_ID");
    let standing = format!("{tag}-standing-$ZEROSHOT_NODE_INSTANCE_ID");
    let publish = publish_command(&once, &standing);
    let command = format!(
        "touch {barrier}/ready-$ZEROSHOT_NODE_INSTANCE_ID; \
         until test \"$(find {barrier} -name 'ready-*' | wc -l)\" -ge 2; do sleep .1; done; \
         {publish} && touch {barrier}/published-$ZEROSHOT_NODE_INSTANCE_ID; \
         until test \"$(find {barrier} -name 'published-*' | wc -l)\" -ge 2; do sleep .1; done"
    );
    let instruction = format!(
        "Your FIRST action must be one native shell call running exactly `{command}`. \
         Shell variables MUST be expanded by the shell. Wait for completion. Then {RECEIVE}"
    );
    let mut left = fixture
        .runner
        .start(fixture.request("peer", (11, 1), &instruction))
        .await
        .unwrap_or_else(|error| panic!("live peer start failed: {error}"));
    let mut right = fixture
        .runner
        .start(fixture.request("peer", (12, 2), &instruction))
        .await
        .unwrap_or_else(|error| panic!("live peer start failed: {error}"));
    let (left, right) = tokio::join!(completion(&mut left), completion(&mut right));
    for (completion, other) in [(left, 12), (right, 11)] {
        assert_eq!(
            messages(completion),
            vec![
                format!("{tag}-once-{other}"),
                format!("{tag}-standing-{other}")
            ]
        );
    }
    let mut expected = [11, 12]
        .into_iter()
        .flat_map(|id| [format!("{tag}-once-{id}"), format!("{tag}-standing-{id}")])
        .collect::<Vec<_>>();
    expected.sort();
    assert_eq!(fixture.turn("tail", (13, 3), RECEIVE).await, expected);
    fixture.runner.close_run(&fixture.run).await;
    let entries = fixture.entries().await;
    let origins = entries
        .iter()
        .filter_map(|v| match &v.content {
            RunVectorContent::Steer {
                network: Some(metadata),
                ..
            } => Some((v.id.clone(), metadata.origin.node_instance)),
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(origins.len(), 4);
    let mut count = 0;
    for vector in &entries {
        if let RunVectorContent::SteerReceipt {
            steer_id,
            node_instance,
            ..
        } = &vector.content
        {
            assert_ne!(origins[steer_id], *node_instance);
            count += 1;
        }
    }
    assert_eq!(count, 8);
}
