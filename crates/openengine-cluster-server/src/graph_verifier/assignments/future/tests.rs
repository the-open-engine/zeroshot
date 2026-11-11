use super::*;
use std::cell::Cell;

struct Snapshot {
    condition: Guard,
    reads: Cell<usize>,
    selection_count: usize,
    constrained_item: Option<u64>,
}
impl ControlSnapshot for Snapshot {
    fn is_fixed(&self, _: &NodeName, _: &[u64]) -> bool {
        false
    }
    fn fixed_count(&self, _: (&ControlSelector, &[EnumLabel], bool), _: &[u64]) -> Option<u64> {
        None
    }
    fn retained(&self, _: &NodeName, _: &[u64], _: usize) -> Option<bool> {
        None
    }
    fn label(&self, _: &ControlSelector, _: &[u64]) -> Option<EnumLabel> {
        None
    }
    fn map_progress(&self, _: &MapNode, _: &[u64]) -> MapProgress {
        MapProgress {
            cardinality: MapCardinality::fixed(if self.constrained_item.is_some() {
                2
            } else {
                0
            }),
            known_items: self.constrained_item.into_iter().collect(),
        }
    }
    fn scoped_guards<'a>(&'a self, scope: &'a [u64]) -> Box<dyn Iterator<Item = &'a Guard> + 'a> {
        Box::new(
            self.constrained_item
                .filter(|index| scope == [*index])
                .map(|_| &self.condition)
                .into_iter(),
        )
    }
    fn selection_guards(&self) -> Box<dyn Iterator<Item = &Guard> + '_> {
        Box::new(
            std::iter::repeat_n(&self.condition, self.selection_count)
                .inspect(|_| self.reads.set(self.reads.get() + 1)),
        )
    }
}

#[test]
fn selector_preparation_stops_reading_when_the_shared_work_budget_is_spent() {
    let branch = step_body();
    let query: Guard = serde_json::from_value(serde_json::json!({"kind":"in","value":{"name":"a","source":"error","field":null},"labels":["crash"]})).expect("valid guard");
    let snapshot = Snapshot {
        condition: Guard::All {
            guards: openengine_cluster_protocol::NonEmptyVec::new(vec![query.clone(); 128])
                .expect("nonempty conditions"),
        },
        reads: Cell::new(0),
        selection_count: 100_000,
        constrained_item: None,
    };
    assert!(guard_has_possible_future(&branch, &query, &snapshot, &[]));
    assert!(
        snapshot.reads.get() > 0 && snapshot.reads.get() < 100_000,
        "budget refusal must stop before collecting the remaining guards"
    );
}

#[test]
fn nonconvergence_preserves_unknown_foreign_controls() {
    let body = step_body();
    let query:Guard=serde_json::from_value(serde_json::json!({"kind":"in","value":{"name":"foreign","source":"error","field":null},"labels":["crash"]})).expect("valid query");
    let until = Guard::Any {
        guards: NonEmptyVec::new(vec![
            query.clone(),
            Guard::Not {
                guard: Box::new(query.clone()),
            },
        ])
        .expect("nonempty"),
    };
    let snapshot = Snapshot {
        condition: query,
        reads: Cell::new(0),
        selection_count: 0,
        constrained_item: None,
    };
    assert!(loop_reentry_possible(
        &body,
        (Some(&until), None),
        &snapshot,
        &[]
    ));
    assert_eq!(snapshot.reads.get(), 0);
}

fn step_body() -> GraphNode {
    serde_json::from_value(serde_json::json!({"kind":"step","name":"a","worker":"worker.test@1","input":{"kind":"null"},"output":{"kind":"null"},"inputBindings":[],"writeBindings":[],"attempts":1})).expect("valid branch")
}

#[test]
fn residual_alternatives_share_one_engineering_bound() {
    let owner = ProofWork::default();
    let first = owner.clone();
    let second = owner.clone();
    assert!(first.charge(ProofWork::LIMIT as usize - 1).is_some());
    assert!(second.charge(1).is_some());
    assert!(owner.charge(1).is_none());
    assert!(first.charge(1).is_none());
    assert!(second.charge(1).is_none());
}

#[test]
fn convergence_constrains_only_its_own_item_before_aggregation() {
    let branch:GraphNode=serde_json::from_value(serde_json::json!({
        "kind":"map","name":"outer","state":{"kind":"null"},
        "body":{"kind":"loop","name":"repeat","state":{"kind":"null"},
            "body":{"kind":"verifier","name":"a","worker":"worker.test@1",
                "input":{"kind":"null"},"output":{"kind":"null"},
                "inputBindings":[],"writeBindings":[],"attempts":1,
                "diagnostic":{"kind":"record","fields":{}},"signals":{"verdict":["accepted","rejected"]}},
            "maxIterations":2,"until":{"kind":"in","value":{"name":"a","source":"signal","field":"verdict"},"labels":["accepted"]},
            "promotedStatePaths":[]},
        "over":{"source":"state","path":["items"]},"maxItems":1,"promotedStatePaths":[]
    })).expect("valid branch");
    let condition:Guard=serde_json::from_value(serde_json::json!({"kind":"k_of_map","count":1,"value":{"name":"repeat","source":"group","field":"terminated"},"labels":["converged"]})).expect("valid constraint");
    for constrained_item in [0, 1] {
        let snapshot = Snapshot {
            condition: condition.clone(),
            reads: Cell::new(0),
            selection_count: 1,
            constrained_item: Some(constrained_item),
        };
        for count in [1, 2] {
            let query:Guard=serde_json::from_value(serde_json::json!({"kind":"k_of_map","count":count,"value":{"name":"a","source":"signal","field":"verdict"},"labels":["rejected"]})).expect("valid query");
            assert_eq!(
                guard_has_possible_future(&branch, &query, &snapshot, &[]),
                count == 1,
                "item={constrained_item} count={count}"
            );
        }
    }
}
