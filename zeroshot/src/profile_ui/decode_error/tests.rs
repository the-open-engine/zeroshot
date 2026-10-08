use super::*;
use crate::native_v2_cli::{BuiltinGraphTemplate, TemplateDelivery};
use crate::profile_ui::ProfileDocument;
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::json;

fn graph() -> Value {
    serde_json::to_value(
        BuiltinGraphTemplate::SingleWorker
            .materialize(TemplateDelivery::None)
            .assert_value(),
    )
    .assert_value()
}

fn runtime(harness: &str, provider: &str, size: Option<&str>, model: &str) -> Value {
    let mut runtime = json!({
        "harness":harness, "provider":provider, "size":size,
        "nodes":{"worker":{"kind":"agent","model":model}}
    });
    if size.is_none() {
        runtime.as_object_mut().assert_value().remove("size");
    }
    runtime
}

fn decode_document(runtime: Value) -> Result<ProfileDocument, DecodeError> {
    decode(
        json!({"graph":graph(),"runtime":runtime})
            .to_string()
            .as_bytes(),
    )
}

#[test]
fn field_problems_name_the_field_in_plain_words() {
    let mut listed = runtime("codex", "openai", Some("small"), "m");
    listed["nodes"] = json!([]);
    let mut dotted = runtime("codex", "openai", Some("small"), "");
    dotted["nodes"] = json!({"worker.a":{"kind":"agent","model":""}});
    let cases = [
        (
            runtime("", "", Some("small"), ""),
            "runtime.harness",
            "Choose a harness.",
        ),
        (
            runtime("codex", "", Some("small"), "m"),
            "runtime.provider",
            "Choose a provider.",
        ),
        (
            runtime("codex", "openai", Some("small"), ""),
            "runtime.nodes.worker.model",
            "Choose a model for `worker`.",
        ),
        (
            runtime("codex", "openai", None, "m"),
            "runtime.size",
            "Choose a size.",
        ),
        (
            runtime("codex", "nope", Some("small"), "m"),
            "runtime.provider",
            "Provider `nope` is not supported. Expected one of openai, openrouter, gateway, bedrock.",
        ),
        (listed, "runtime.nodes", "Nodes must be a map."),
        (
            dotted,
            "runtime.nodes.worker.a.model",
            "Choose a model for `worker.a`.",
        ),
    ];
    for (runtime, field, message) in cases {
        let Err(DecodeError::Field(problem)) = decode_document(runtime) else {
            panic!("expected a field problem at {field}");
        };
        assert_eq!(
            (problem.field.as_str(), problem.message.as_str()),
            (field, message)
        );
        assert!(!problem.detail.is_empty() && !problem.detail.contains(" at line "));
    }
}

#[test]
fn malformed_json_and_trailing_content_are_rejected_without_a_field() {
    let document = json!({
        "graph":graph(),
        "runtime":runtime("codex", "openai", Some("small"), "m"),
    });
    assert!(decode::<ProfileDocument>(document.to_string().as_bytes()).is_ok());
    for malformed in [
        "{".to_owned(),
        format!("{document} {{}}"),
        format!("{document} stray"),
    ] {
        let Err(DecodeError::Malformed(message)) = decode::<ProfileDocument>(malformed.as_bytes())
        else {
            panic!("expected malformed JSON for {malformed:.40}");
        };
        assert!(message.starts_with("Invalid profile JSON"));
    }
}
