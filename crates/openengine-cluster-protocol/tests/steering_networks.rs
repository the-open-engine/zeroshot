use openengine_cluster_protocol::{
    CompiledGraphIr, GraphSpec, RunSteerParams, RunVector, RunVectorContent, RunVectorSummary,
    SteeringMode,
};
use openengine_cluster_testkit::admission::{compiled_from_graph_fixture, graph_fixture};
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::{json, Value};

fn graph_value() -> Value {
    serde_json::to_value(graph_fixture("work", json!({"kind":"null"}))).assert_value()
}

#[test]
fn networks_are_optional_canonical_sets_and_omit_empty_graph_maps() {
    let original = graph_value();
    let plain: GraphSpec = serde_json::from_value(original.clone()).assert_value();
    assert!(plain.networks.is_empty());
    assert_eq!(serde_json::to_value(&plain).assert_value(), original);
    let identity = compiled_from_graph_fixture(&plain)
        .identity()
        .assert_value();
    let mut explicit = original.clone();
    explicit["networks"] = json!({});
    let graph: GraphSpec = serde_json::from_value(explicit).assert_value();
    assert_eq!(
        compiled_from_graph_fixture(&graph)
            .identity()
            .assert_value(),
        identity
    );

    let mut authored = original;
    authored["networks"] = json!({"team":{"members":["work","peer","work"]}});
    let graph: GraphSpec = serde_json::from_value(authored).assert_value();
    assert!(
        graph
            .networks
            .values()
            .next()
            .assert_value()
            .publishers
            .is_empty()
    );
    let compiled = compiled_from_graph_fixture(&graph);
    assert_ne!(compiled.identity().assert_value(), identity);
    let mut ordered = serde_json::to_value(&compiled).assert_value();
    ordered["networks"] = json!({"team":{"members":["peer","work"],"publishers":[]}});
    let equivalent: CompiledGraphIr = serde_json::from_value(ordered).assert_value();
    assert_eq!(
        compiled.canonical_bytes().assert_value(),
        equivalent.canonical_bytes().assert_value()
    );
}

#[test]
fn network_names_use_the_same_rust_and_schema_identifier_bounds() {
    let schema = serde_json::to_value(schemars::schema_for!(GraphSpec)).assert_value();
    let validator = jsonschema::validator_for(&schema).assert_value();
    for invalid in ["bad/name".to_owned(), "a".repeat(129)] {
        let mut value = graph_value();
        value["networks"] = json!({invalid:{"members":["work"]}});
        assert!(serde_json::from_value::<GraphSpec>(value.clone()).is_err());
        assert!(!validator.is_valid(&value));
    }
}

#[test]
fn legacy_human_steers_default_to_standing_and_network_metadata_survives_projections() {
    let params: RunSteerParams =
        serde_json::from_value(json!({"runId":"run","text":"direction"})).assert_value();
    assert_eq!(params.mode, SteeringMode::Standing);
    assert_eq!(
        serde_json::to_value(params).assert_value()["mode"],
        "standing"
    );
    let legacy = json!({"id":"message","runId":"run","createdAt":1762905600000_u64,
        "kind":"steer","text":"direction"});
    let vector: RunVector = serde_json::from_value(legacy.clone()).assert_value();
    assert!(matches!(
        vector.content,
        RunVectorContent::Steer {
            mode: SteeringMode::Standing,
            network: None,
            ..
        }
    ));
    for mode in ["once", "standing"] {
        let mut peer = legacy.clone();
        peer["mode"] = json!(mode);
        peer["network"] = json!({"networks":["team"],"origin":{
            "node":"work","nodeInstance":7,"execution":"execution/7"
        }});
        let vector: RunVector = serde_json::from_value(peer.clone()).assert_value();
        vector.validate().assert_value();
        assert_eq!(serde_json::to_value(&vector).assert_value(), peer);
        let summary = serde_json::to_value(RunVectorSummary::from(vector)).assert_value();
        assert_eq!(summary["mode"], mode);
        assert_eq!(summary["network"], peer["network"]);
        assert!(summary.get("text").is_none());
    }
    for network in [
        json!({"networks":[],"origin":{"node":"work","nodeInstance":7,"execution":"execution/7"}}),
        json!({"networks":["team"],"origin":{"node":"work","nodeInstance":0,"execution":"execution/7"}}),
    ] {
        let mut invalid = legacy.clone();
        invalid["network"] = network;
        let vector: RunVector = serde_json::from_value(invalid).assert_value();
        assert!(vector.validate().is_err());
    }
}
