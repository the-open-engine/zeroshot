use super::*;
use async_trait::async_trait;
use openengine_cluster_protocol::{WorkerDescriptor, WorkerRef};
use openengine_cluster_server::worker_registry::{WorkerRegistry, WorkerRegistryError};

struct SignalWorkers(&'static str);
#[async_trait]
impl WorkerRegistry for SignalWorkers {
    async fn resolve(&self, worker: &WorkerRef) -> Result<WorkerDescriptor, WorkerRegistryError> {
        let registry = reducer_test_support::TestWorkers { rich_outputs: true };
        if worker.as_str() != "worker.signal@1" {
            return registry.resolve(worker).await;
        }
        let original = registry
            .resolve(&"worker.verify@1".parse().unwrap())
            .await?;
        let mut descriptor = serde_json::to_value(original).unwrap();
        descriptor["worker"] = json!("worker.signal@1");
        descriptor["contract"]["verifier"]["signals"] = json!({self.0:["accepted","rejected"]});
        Ok(serde_json::from_value(descriptor).unwrap())
    }
}

fn producer(name: &str, field: &str) -> Value {
    let mut node = verifier(name, 1);
    node["worker"] = json!("worker.signal@1");
    node["signals"] = json!({field:["accepted","rejected"]});
    node
}

fn with_field(mut guard: Value, field: &str) -> Value {
    guard["value"]["field"] = json!(field);
    guard
}

fn observed(id: u64, name: &str, signal: (&str, &str), scope: Vec<u64>) -> DurableExecution {
    let (field, label) = signal;
    let mut execution = entry(id, name, label, scope);
    if let DurableExecutionState::Settled {
        outcome: WorkerOutcome::Verifier { signals, .. },
        ..
    } = &mut execution.state
    {
        *signals = [(field.parse().unwrap(), label.parse().unwrap())]
            .into_iter()
            .collect();
    }
    execution
}

async fn field_graph(
    field: &'static str,
    mut branch: Value,
    guard: Value,
    depth: usize,
) -> VerifiedGraph {
    for index in 0..depth {
        branch = mapped(branch, 1);
        branch["name"] = json!(format!("field_map_{index}"));
    }
    let mut root = root_with_join(
        vec![branch, verifier("b", 1)],
        json!({"kind":"first","when":either(guard)}),
        &state(),
    );
    root["children"][1] = no_satisfier();
    let spec: GraphSpec = serde_json::from_value(json!({"profile":"openengine.graph.full/v1","initialInput":state(),"policy":{"policy":"policy.test@1","default":"deny"},"root":root})).unwrap();
    ProductionGraphVerifier::new(SignalWorkers(field))
        .verify(&spec)
        .await
        .unwrap()
}

const FIELDS: &[&str] = &[
    "review",
    "__map_body_completed",
    "__map_write.0",
    "__retained.0",
    "joined",
    "raced",
    "terminated",
];

#[tokio::test]
async fn authored_signal_names_keep_their_domains_at_every_map_scope() {
    for &field in FIELDS {
        for depth in 0..=2 {
            let g = field_graph(
                field,
                producer("a", field),
                with_field(signal("a", "accepted", depth > 0), field),
                depth,
            )
            .await;
            let input = json!({"items":[null]});
            let mut history = vec![entry(1, "b", "rejected", vec![])];
            let pending = one_slot(&g, &input, &history);
            assert!(
                pending.terminal.is_none(),
                "{field}, depth {depth}: {pending:#?}"
            );
            assert!(pending.decisions.iter().any(
                |d| matches!(d,Decision::Dispatch {occurrence,..} if occurrence.node.as_str()=="a")
            ));
            history.push(observed(2, "a", (field, "accepted"), vec![0; depth]));
            assert!(matches!(
                one_slot(&g, &input, &history).terminal,
                Some(TerminalProjection::Succeeded { .. })
            ));
        }
    }
}

#[tokio::test]
async fn frozen_signals_named_like_group_controls_preserve_selected_routes() {
    for &field in FIELDS {
        for depth in 0..=2 {
            let choice = json!({"kind":"choice","name":"choose","state":state(),"branches":[{"when":with_field(signal("gate","accepted",false),field),"node":verifier("a",1)}],"otherwise":step("noop",1),"promotedStatePaths":[]});
            let left = seq(
                "left",
                vec![producer("gate", field), choice, step("tail", 1)],
                &state(),
            );
            let g = field_graph(field, left, signal("a", "accepted", depth > 0), depth).await;
            let scope = vec![0; depth];
            let input = json!({"items":[null]});
            let history = vec![
                entry(1, "b", "rejected", vec![]),
                observed(2, "gate", (field, "rejected"), scope.clone()),
                done(3, "noop", scope.clone()),
            ];
            let pending = one_slot(&g, &input, &history);
            assert_pending_dispatch(&pending, "tail");
            let mut complete = history;
            complete.push(done(4, "tail", scope.clone()));
            assert_no_satisfier(&one_slot(&g, &input, &complete));
            let mut accepted = vec![
                entry(1, "b", "rejected", vec![]),
                observed(2, "gate", (field, "accepted"), scope.clone()),
            ];
            assert!(one_slot(&g, &input, &accepted).terminal.is_none());
            accepted.extend([
                entry(3, "a", "accepted", scope.clone()),
                done(4, "tail", scope),
            ]);
            assert!(matches!(
                one_slot(&g, &input, &accepted).terminal,
                Some(TerminalProjection::Succeeded { .. })
            ));
        }
    }
}
