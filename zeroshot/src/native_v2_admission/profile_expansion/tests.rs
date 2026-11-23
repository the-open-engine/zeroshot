use super::*;
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
use serde_json::{Value, json};

type Catalog = BTreeMap<RunProfileName, (GraphSpec, RuntimePlan)>;

fn definition(root: Value) -> (GraphSpec, RuntimePlan) {
    let root = if root["kind"] == "profile" {
        json!({"kind":"seq","name":"scope","state":{"kind":"null"},"promotedStatePaths":[],"children":[root, done("complete")]})
    } else {
        root
    };
    (
        serde_json::from_value(json!({
            "profile":"openengine.graph.full/v1", "initialInput":{"kind":"null"},
            "policy":{"policy":"policy.native-v2@1","default":"deny"}, "root":root
        }))
        .assert_value(),
        serde_json::from_value(
            json!({"harness":"codex","provider":"openai","size":"host","nodes":{}}),
        )
        .assert_value(),
    )
}

fn import(name: &str, profile: &str) -> Value {
    json!({"kind":"profile","name":name,"profile":profile})
}

fn done(name: &str) -> Value {
    json!({"kind":"succeed","name":name,"output":{"kind":"null"},"bindings":[]})
}

fn expand(
    parent: (GraphSpec, RuntimePlan),
    catalog: &Catalog,
) -> Result<(GraphSpec, RuntimePlan), ProfileExpansionError> {
    let root = RunProfileName::new("parent").assert_value();
    expand_profile_imports(parent.0, parent.1, Some(&root), |name| {
        Ok(catalog.get(name).cloned())
    })
}

#[tokio::test]
async fn immutable_catalog_nested_imports_preserve_lanes_and_admit() {
    let mut leaf = definition(
        json!({"kind":"seq","name":"root","state":{"kind":"null"},"promotedStatePaths":[],"children":[
            {"kind":"step","name":"worker","worker":"worker.task@1","instructions":"Exercise the imported agent.","input":{"kind":"null"},"output":{"kind":"null"},"inputBindings":[],"writeBindings":[],"attempts":1},
            done("done")
        ]}),
    );
    leaf.1 = serde_json::from_value(
        json!({"harness":"claude","provider":"anthropic","size":"host","nodes":{
            "worker":{"kind":"agent","model":"fixture-model"}
        }}),
    )
    .assert_value();
    leaf.0.networks =
        serde_json::from_value(json!({"team":{"members":["worker"],"publishers":["worker"]}}))
            .assert_value();
    let catalog = BTreeMap::from([
        (
            RunProfileName::new("middle").assert_value(),
            definition(import("inner", "leaf")),
        ),
        (RunProfileName::new("leaf").assert_value(), leaf),
    ]);
    let parent = definition(import("outer", "middle"));
    let frozen = expand(parent.clone(), &catalog).assert_value();
    assert_eq!(expand(parent, &catalog).assert_value(), frozen);
    let name = NodeName::new("outer.inner.worker").assert_value();
    assert_eq!(
        frozen.1.effective_lane(&frozen.1.nodes()[&name]),
        Some(openengine_cluster_protocol::RuntimeLane::new("claude", "anthropic").assert_value())
    );
    assert_eq!(
        frozen.0.networks[&NodeName::new("outer.inner.team").assert_value()].members,
        BTreeSet::from([name])
    );
    let admission = super::super::NativeV2Admission
        .validate_profile(&frozen.0, &frozen.1, super::super::DeliveryPolicy::Optional)
        .await;
    assert!(admission.is_ok(), "{admission:?}");
}

#[test]
fn missing_cycles_untrusted_expansion_and_policy_fail_closed() {
    let parent = definition(import("call", "leaf"));
    let missing = expand(parent.clone(), &Catalog::new()).assert_error();
    assert!(missing.to_string().contains("missing profile leaf"));
    let mut catalog = Catalog::from([(
        RunProfileName::new("leaf").assert_value(),
        definition(import("again", "parent")),
    )]);
    assert!(
        expand(parent.clone(), &catalog)
            .assert_error()
            .to_string()
            .contains("cycle parent → leaf → parent")
    );
    catalog.insert(
        RunProfileName::new("leaf").assert_value(),
        definition(done("done")),
    );
    let expanded = expand(parent.clone(), &catalog).assert_value();
    assert!(
        expand(expanded, &catalog)
            .assert_error()
            .to_string()
            .contains("must omit its expansion")
    );
    let mut untrusted = parent.clone();
    let GraphNode::Seq(group) = &mut untrusted.0.root else {
        panic!("sequence")
    };
    let GraphNode::Profile(call) = &mut group.children.as_mut_slice()[0] else {
        panic!("profile")
    };
    call.initial_input = Some(serde_json::from_value(json!({"kind":"null"})).assert_value());
    assert!(
        expand(untrusted, &catalog)
            .assert_error()
            .to_string()
            .contains("must omit its expansion")
    );
    catalog.values_mut().next().assert_value().0.policy =
        serde_json::from_value(json!({"policy":"other@1","default":"deny"})).assert_value();
    assert!(
        expand(parent, &catalog)
            .assert_error()
            .to_string()
            .contains("parent graph policy")
    );
}

#[test]
fn imported_bytes_nodes_and_depth_remain_bounded() {
    let mut root = done("done");
    for index in 0..63 {
        root = json!({"kind":"seq","name":format!("group{index}"),"state":{"kind":"null"},"promotedStatePaths":[],"children":[root]});
    }
    let mut catalog =
        Catalog::from([(RunProfileName::new("leaf").assert_value(), definition(root))]);
    let parent = definition(import("call", "leaf"));
    assert!(
        expand(parent.clone(), &catalog)
            .assert_error()
            .to_string()
            .contains("depth or node limit")
    );
    let children: Vec<_> = (0..FULL_V1_MAX_GRAPH_NODES)
        .map(|i| done(&format!("node{i}")))
        .collect();
    catalog.values_mut().next().assert_value().0.root = serde_json::from_value(json!({"kind":"seq","name":"root","state":{"kind":"null"},"promotedStatePaths":[],"children":children})).assert_value();
    assert!(
        expand(parent.clone(), &catalog)
            .assert_error()
            .to_string()
            .contains("depth or node limit")
    );
    let mut leaf = definition(done("done"));
    let binding: NodeRuntimeBinding = serde_json::from_value(json!({"kind":"agent","model":"model","harnessConfig":{"options":{"blob":"x".repeat(60_000)}}})).assert_value();
    for index in 0..=(openengine_cluster_protocol::MAX_REQUEST_RESPONSE_BYTES / 60_000) {
        leaf.1.nodes_mut().insert(
            NodeName::new(format!("worker{index}")).assert_value(),
            binding.clone(),
        );
    }
    catalog.insert(RunProfileName::new("leaf").assert_value(), leaf);
    assert!(
        expand(parent.clone(), &catalog)
            .assert_error()
            .to_string()
            .contains("request size limit")
    );
    let half = &mut catalog.values_mut().next().assert_value().1;
    while half.nodes().len() > openengine_cluster_protocol::MAX_REQUEST_RESPONSE_BYTES / 120_000 + 1
    {
        half.nodes_mut().pop_last();
    }
    expand(parent, &catalog).assert_value();
    let repeated = definition(
        json!({"kind":"seq","name":"root","state":{"kind":"null"},"promotedStatePaths":[],"children":[
            import("first", "leaf"), import("second", "leaf"), done("done")
        ]}),
    );
    assert!(
        expand(repeated, &catalog)
            .assert_error()
            .to_string()
            .contains("request size limit")
    );
    let bounded = ProfileExpansionError::new(format!("unsafe\n{}", "🦀".repeat(5000)));
    assert!(bounded.to_string().len() <= 4096);
    assert!(!bounded.to_string().contains('\n'));
}
