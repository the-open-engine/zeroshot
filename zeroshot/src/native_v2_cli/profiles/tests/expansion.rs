use super::*;
use openengine_cluster_protocol::{MAX_REQUEST_RESPONSE_BYTES, NodeName, WorkerRef};
use serde_json::Value;

fn signal(name: &str) -> Value {
    json!({"name": name, "source": "signal", "field": "verdict"})
}

fn signal_guard(name: &str) -> Value {
    json!({"kind": "in", "value": signal(name), "labels": ["ok"]})
}

fn verifier(name: &str, worker: &str) -> Value {
    json!({
        "kind": "verifier", "name": name, "worker": worker,
        "input": {"kind": "null"}, "output": {"kind": "null"},
        "inputBindings": [], "writeBindings": [], "attempts": 1,
        "signals": {"verdict": ["ok"]}, "diagnostic": {"kind": "null"}
    })
}

fn controlled_child(worker_name: &str) -> RunProfileSetRequest {
    let mut child = nested_profile_request("leaf", 2);
    let mut graph = serde_json::to_value(&child.graph).assert_value();
    let mut validator = verifier(worker_name, "worker.check@2");
    validator["writeBindings"] = json!([{
        "target": ["verdict"],
        "value": {"node": worker_name, "channel": "signal", "path": ["verdict"]}
    }]);
    let null_state = json!({"kind": "null"});
    let ask = crate::native_v2_request::ASK_WORKER_REF;
    graph["networks"] = json!({
        "peer": {"members":[worker_name],"publishers":[worker_name]}
    });
    graph["root"]["children"] = json!([
        {
            "kind": "loop", "name": "round", "state": null_state, "maxIterations": 2,
            "promotedStatePaths": [],
            "until": {"kind": "all", "guards": [
                {"kind": "any", "guards": [
                    signal_guard(worker_name), {"kind": "not", "guard": signal_guard("ask")}
                ]},
                {"kind": "k_of_n", "count": 1, "values": [signal(worker_name), signal("ask")], "labels": ["ok"]}
            ]},
            "body": {
                "kind": "par", "name": "race", "state": null_state, "promotedStatePaths": [],
                "join": {"kind": "first", "when": signal_guard("ask")},
                "branches": [
                    {
                        "kind": "map", "name": "items", "state": null_state, "promotedStatePaths": [],
                        "over": {"source": "state", "path": ["items"]}, "maxItems": 1, "body": validator
                    },
                    verifier("ask", ask)
                ]
            }
        },
        {
            "kind": "choice", "name": "finish", "state": null_state, "promotedStatePaths": [],
            "branches": [{
                "when": {"kind": "k_of_map", "count": 1, "value": signal(worker_name), "labels": ["ok"]},
                "node": {"kind": "fail", "name": "stop", "reason": "test_stop"}
            }],
            "otherwise": graph["root"]["children"][0]
        }
    ]);
    child.graph = serde_json::from_value(graph).expect("linker fixture must parse");
    child.runtime.nodes_mut().insert(
        NodeName::new(worker_name).assert_value(),
        serde_json::from_value(json!({"kind": "agent", "model": "test-model"})).assert_value(),
    );
    child.runtime.nodes_mut().insert(
        NodeName::new("ask").assert_value(),
        openengine_cluster_protocol::NodeRuntimeBinding::Request {},
    );
    child
}

#[test]
fn profile_expansion_qualifies_control_edges_bindings_and_networks() {
    let (_directory, store) = temporary_store();
    let long_name = "w".repeat(128);
    store.set(controlled_child(&long_name)).assert_value();
    let parent = importing_profile("parent", "leaf", &["imported"]);
    let (graph, runtime) = store
        .expand(parent.graph, parent.runtime, Some(&parent.name))
        .assert_value();
    let graph = serde_json::to_value(graph).assert_value();
    let body = &graph["root"]["children"][0]["body"];
    let round = &body["children"][0];
    let race = &round["body"];
    let validator = &race["branches"][0]["body"];
    let name = validator["name"].as_str().assert_value();
    let worker = WorkerRef::new(validator["worker"].as_str().assert_value()).assert_value();

    assert!(name.starts_with("profile_") && name.len() <= 128);
    assert_ne!(worker.as_str(), "worker.check@2");
    assert_eq!(validator["writeBindings"][0]["value"]["node"], name);
    assert_eq!(
        race["branches"][1]["worker"],
        crate::native_v2_request::ASK_WORKER_REF
    );
    assert!(
        runtime
            .nodes()
            .contains_key(&NodeName::new(name).assert_value())
    );
    assert!(matches!(
        runtime.nodes()[&NodeName::new("imported.ask").assert_value()],
        openengine_cluster_protocol::NodeRuntimeBinding::Request {}
    ));
    assert_eq!(race["join"]["when"], signal_guard("imported.ask"));
    assert_eq!(round["until"]["guards"][0]["guards"][0], signal_guard(name));
    assert_eq!(
        round["until"]["guards"][0]["guards"][1]["guard"],
        signal_guard("imported.ask")
    );
    assert_eq!(
        round["until"]["guards"][1]["values"],
        json!([signal(name), signal("imported.ask")])
    );
    assert_eq!(
        body["children"][1]["branches"][0]["when"]["value"],
        signal(name)
    );
    assert_eq!(
        body["children"][1]["branches"][0]["node"]["name"],
        "imported.stop"
    );
    assert_eq!(body["children"][1]["otherwise"]["name"], "imported.done");
    assert_eq!(
        graph["networks"]["imported.peer"],
        json!({"members":[name],"publishers":[name]})
    );
}

#[test]
fn profile_expansion_rejects_conflicting_runtime_and_network_names() {
    let (_directory, store) = temporary_store();
    store.set(controlled_child("check")).assert_value();
    let mut parent = importing_profile("parent", "leaf", &["imported"]);
    let binding = openengine_cluster_protocol::NodeRuntimeBinding::Request {};
    parent
        .runtime
        .nodes_mut()
        .insert(NodeName::new("imported.ask").assert_value(), binding);
    assert!(matches!(
        store.expand(parent.graph.clone(), parent.runtime.clone(), Some(&parent.name)),
        Err(NativeV2CliError::Usage(message)) if message.contains("duplicate expanded runtime binding imported.ask")
    ));

    parent.runtime.nodes_mut().clear();
    parent.graph.networks.insert(
        NodeName::new("imported.peer").assert_value(),
        serde_json::from_value(json!({"members":["imported.check"]})).assert_value(),
    );
    assert!(matches!(
        store.expand(parent.graph, parent.runtime, Some(&parent.name)),
        Err(NativeV2CliError::Usage(message)) if message.contains("duplicate expanded network imported.peer")
    ));
}

#[test]
fn profile_expansion_checks_the_store_before_accepting_a_body() {
    let (_directory, store) = temporary_store();
    let parent = importing_profile("parent", "leaf", &["imported"]);
    assert!(matches!(
        store.expand(parent.graph.clone(), parent.runtime.clone(), None),
        Err(NativeV2CliError::Usage(message)) if message.contains("references missing local profile leaf")
    ));
    store
        .set(profile_request("leaf", "worker", false))
        .assert_value();
    assert!(matches!(
        store.expand(parent.graph.clone(), parent.runtime.clone(), None),
        Err(NativeV2CliError::Usage(message)) if message.contains("must use full/v1 and the parent graph policy")
    ));

    store.set(nested_profile_request("leaf", 1)).assert_value();
    let (expanded, runtime) = store
        .expand(parent.graph.clone(), parent.runtime.clone(), None)
        .assert_value();
    assert!(matches!(
        store.expand(expanded, runtime, None),
        Err(NativeV2CliError::Usage(message)) if message.contains("must omit its expansion")
    ));

    let mut snapshot = store.read().assert_value();
    snapshot.profiles.insert(
        selector("leaf").name,
        to_raw_value(&"x".repeat(MAX_REQUEST_RESPONSE_BYTES)).assert_value(),
    );
    assert!(matches!(
        super::super::expansion::expand(&snapshot, parent.graph, parent.runtime, None),
        Err(NativeV2CliError::Usage(message)) if message.contains("imports exceed the request size limit")
    ));
}
