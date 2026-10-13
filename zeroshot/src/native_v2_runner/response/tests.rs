use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::json;

use super::*;

fn worker_contract() -> NodeResponseContract {
    NodeResponseContract::Worker {
        output: serde_json::from_value(json!({
            "kind": "record",
            "fields": {
                "answer": { "type": { "kind": "integer" }, "required": true }
            }
        }))
        .assert_value(),
    }
}

#[test]
fn workspace_and_verifier_guidance_are_runtime_owned() {
    let instructions = NodeInstructions::new("Assess a custom project's behavior.").assert_value();
    let input = json!({"task":"custom review"});
    let verifier = NodeResponseContract::Verifier {
        output: PayloadType::Null,
        signals: BTreeMap::new(),
        diagnostic: PayloadType::Null,
    };
    let prompt = render_agent_prompt(&instructions, &input, &verifier).assert_value();
    assert!(prompt.contains(instructions.as_str()));
    assert!(!prompt.contains("Runtime-owned workspace setup guidance:"));
    assert!(prompt.contains("Do not run setup or dependency-install commands"));
    assert!(prompt.contains("other nodes may run concurrently"));
    assert!(prompt.contains("Do not modify reviewed material"));
    assert!(prompt.contains("create artifacts"));
    assert!(prompt.contains("missing declared dependency is an environment blocker"));
    assert!(prompt.contains("do not request unrelated code repairs"));
    assert!(prompt.contains(&input.to_string()));
    assert!(prompt.contains(&serde_json::to_string(&verifier).assert_value()));
    let worker = render_agent_prompt(&instructions, &input, &worker_contract()).assert_value();
    assert!(worker.contains("Runtime-owned workspace setup guidance:"));
    assert!(worker.contains("manifest/lockfile dependencies in the checkout"));
    assert!(worker.contains("Wait for setup to finish and check exit status"));
    assert!(worker.contains("`$ZEROSHOT_TOOLS/bin`"));
    assert!(worker.contains("`/tmp` (possibly `noexec`)"));
    assert!(!worker.contains("Runtime-owned verifier guidance:"));
}

#[test]
fn schema_rendering_shows_the_provider_schema_and_never_the_contract() {
    let instructions = NodeInstructions::new("Change the workspace.").assert_value();
    let input = json!({"task":"change the workspace"});
    let contract = worker_contract();
    let schema = contract.provider_schema(ProviderSchemaDialect::Standard);
    let schema_json = serde_json::to_string(&schema).assert_value();

    let prompt = render_agent_prompt_with_schema(
        &instructions,
        &input,
        &contract,
        ProviderSchemaDialect::Standard,
    )
    .assert_value();
    assert!(prompt.contains(instructions.as_str()));
    assert!(prompt.contains(&input.to_string()));
    assert!(prompt.contains("Runtime-owned response schema:"));
    assert!(prompt.contains(&schema_json));
    assert!(!prompt.contains("Runtime-owned response contract:"));
    assert!(!prompt.contains("\"kind\":\"worker\""));
    assert!(prompt.contains("never return the schema itself"));

    let error = NodeResponseError::new("output is not an integer".to_owned());
    let correction =
        render_agent_correction_with_schema(&contract, &error, ProviderSchemaDialect::Standard)
            .assert_value();
    assert!(correction.contains("output is not an integer"));
    assert!(correction.contains("Response schema:"));
    assert!(correction.contains(&schema_json));
    assert!(!correction.contains("Response contract:"));
    assert!(!correction.contains("\"kind\":\"worker\""));
}

#[test]
fn schema_rendering_reflects_the_selected_dialect() {
    let instructions = NodeInstructions::new("Change the workspace.").assert_value();
    // An optional integer field distinguishes the dialects: strict mode requires the field and
    // admits an explicit null, while the neutral dialect leaves it optional.
    let contract = NodeResponseContract::Worker {
        output: serde_json::from_value(json!({
            "kind": "record",
            "fields": {
                "answer": { "type": { "kind": "integer" }, "required": true },
                "note": { "type": { "kind": "integer" }, "required": false }
            }
        }))
        .assert_value(),
    };
    let strict = render_agent_prompt_with_schema(
        &instructions,
        &Value::Null,
        &contract,
        ProviderSchemaDialect::OpenAiStrict,
    )
    .assert_value();
    let neutral = render_agent_prompt_with_schema(
        &instructions,
        &Value::Null,
        &contract,
        ProviderSchemaDialect::Standard,
    )
    .assert_value();
    assert_ne!(strict, neutral);
    assert!(
        strict.contains(
            &serde_json::to_string(&contract.provider_schema(ProviderSchemaDialect::OpenAiStrict))
                .assert_value()
        )
    );
    assert!(
        neutral.contains(
            &serde_json::to_string(&contract.provider_schema(ProviderSchemaDialect::Standard))
                .assert_value()
        )
    );
}

#[test]
fn schema_resolution_correction_carries_the_schema_instead_of_the_contract() {
    let contract = worker_contract();
    let response =
        resolve_agent_response_with_schema(&contract, "not json", ProviderSchemaDialect::Standard)
            .assert_value();
    match response {
        AgentResponse::Correction { prompt, .. } => {
            assert!(prompt.contains("Response schema:"));
            assert!(!prompt.contains("\"kind\":\"worker\""));
        }
        AgentResponse::Complete(_) => panic!("malformed JSON must request a correction"),
    }
}

#[test]
fn dialect_resolution_keeps_the_contract_correction_for_native_schema_lanes() {
    let contract = worker_contract();
    let response = resolve_agent_response_with_dialect(
        &contract,
        "not json",
        ProviderSchemaDialect::OpenAiStrict,
    )
    .assert_value();
    match response {
        AgentResponse::Correction { prompt, .. } => {
            assert!(prompt.contains("Response contract:"));
            assert!(prompt.contains("\"kind\":\"worker\""));
        }
        AgentResponse::Complete(_) => panic!("malformed JSON must request a correction"),
    }
}

#[test]
fn native_schema_lanes_keep_the_contract_prompt_byte_for_byte() {
    // `render_agent_prompt` is the shared renderer for Claude, Codex and Copilot (including the
    // GitHub lane). The Pi schema renderer is additive, so this captured full prompt must stay
    // byte-identical; any drift in the shared renderer fails this exact comparison rather than a
    // substring probe.
    let instructions = NodeInstructions::new("Change the workspace.").assert_value();
    let input = json!({"task":"change the workspace"});
    let contract = worker_contract();
    let prompt = render_agent_prompt(&instructions, &input, &contract).assert_value();
    let expected = concat!(
        "Execute this graph node using the shared workspace.\n",
        "Authored instructions:\n",
        "Change the workspace.\n",
        "Runtime-owned workspace setup guidance:\n",
        "Before returning, follow repository setup and install manifest/lockfile dependencies in the checkout; do not use an ad hoc unpinned list. Wait for setup to finish and check exit status. Leave ignored dependencies there. Put standalone tools under an executable user path: use `$ZEROSHOT_TOOLS/bin` when provided (shared by all nodes), otherwise `$HOME/.local/bin`. Do not install tools in `/tmp` (possibly `noexec`).\n",
        "Input JSON:\n",
        "{\"task\":\"change the workspace\"}\n",
        "Runtime-owned response contract:\n",
        "{\"kind\":\"worker\",\"output\":{\"kind\":\"record\",\"fields\":{\"answer\":{\"type\":{\"kind\":\"integer\"},\"required\":true}}}}\n",
        "The response contract describes the required type; never return the contract itself. ",
        "Return only JSON with no Markdown or commentary. The provider response must be exactly ",
        "an object with one field named response. For a worker, response contains the output ",
        "value; an output contract of {\"kind\":\"null\"} requires the literal null inside ",
        "{\"response\":null}. For a verifier, response contains exactly an object with output, ",
        "signals, and diagnostic; every signal must use one of its declared labels."
    );
    assert_eq!(prompt, expected);
    assert!(!prompt.contains("Runtime-owned response schema:"));
}

#[test]
fn agent_response_reports_mechanical_json_and_payload_errors() {
    let contract = worker_contract();
    let malformed = contract
        .parse_agent_response("not json")
        .err()
        .assert_value();
    assert!(
        malformed
            .to_string()
            .starts_with("final response is not valid JSON:")
    );

    assert_eq!(
        contract.parse_agent_response(r#"{"answer":"wrong"}"#),
        Err(NodeResponseError::new(
            "output $.answer must be a integer".to_owned()
        ))
    );
    assert!(matches!(
        contract.parse_agent_response(r#"{"answer":42}"#).assert_value(),
        WorkerOutcome::Verified { output, .. } if output == json!({"answer": 42})
    ));
}

#[test]
fn provider_schema_closes_the_transport_and_preserves_optional_fields() {
    let contract = NodeResponseContract::Worker {
        output: serde_json::from_value(json!({
            "kind": "record",
            "fields": {
                "answer": { "type": { "kind": "integer" }, "required": true },
                "notes": {
                    "type": {
                        "kind": "array",
                        "items": { "kind": "enum", "values": ["ripe", "fresh"] }
                    },
                    "required": false
                }
            }
        }))
        .assert_value(),
    };

    assert_eq!(
        contract.provider_schema(ProviderSchemaDialect::Standard),
        json!({
            "type": "object",
            "properties": {
                "response": {
                    "type": "object",
                    "properties": {
                        "answer": { "type": "integer" },
                        "notes": {
                            "type": "array",
                            "items": {
                                "type": "string",
                                "enum": ["fresh", "ripe"]
                            }
                        }
                    },
                    "required": ["answer"],
                    "additionalProperties": false
                }
            },
            "required": ["response"],
            "additionalProperties": false
        })
    );
}

#[test]
fn provider_schema_covers_every_payload_kind_and_the_verifier_shape() {
    for (payload, expected) in [
        (PayloadType::Null, json!({"type":"null"})),
        (PayloadType::Boolean, json!({"type":"boolean"})),
        (PayloadType::Integer, json!({"type":"integer"})),
        (PayloadType::Number, json!({"type":"number"})),
        (PayloadType::String, json!({"type":"string"})),
    ] {
        let contract = NodeResponseContract::Worker { output: payload };
        assert_eq!(
            contract
                .provider_schema(ProviderSchemaDialect::Standard)
                .pointer("/properties/response")
                .assert_value(),
            &expected
        );
    }

    let verifier = NodeResponseContract::Verifier {
        output: PayloadType::Null,
        signals: BTreeMap::from([(
            FieldName::new("verdict").assert_value(),
            NonEmptyEnumSet::new(vec![
                EnumLabel::new("rejected").assert_value(),
                EnumLabel::new("accepted").assert_value(),
            ])
            .assert_value(),
        )]),
        diagnostic: PayloadType::Boolean,
    };
    let schema = verifier.provider_schema(ProviderSchemaDialect::Standard);
    let response = schema.pointer("/properties/response").assert_value();
    assert_eq!(
        response.pointer("/required").assert_value(),
        &json!(["diagnostic", "output", "signals"])
    );
    assert_eq!(
        response.pointer("/properties/output").assert_value(),
        &json!({"type":"null"})
    );
    assert_eq!(
        response
            .pointer("/properties/signals/properties/verdict")
            .assert_value(),
        &json!({"type":"string", "enum":["accepted", "rejected"]})
    );
    assert_eq!(
        response.pointer("/properties/diagnostic").assert_value(),
        &json!({"type":"boolean"})
    );
}

#[test]
fn openai_schema_requires_every_field_and_normalizes_optional_nulls() {
    let contract = NodeResponseContract::Worker {
        output: serde_json::from_value(json!({
            "kind": "record",
            "fields": {
                "required": { "type": { "kind": "boolean" }, "required": true },
                "optional": { "type": { "kind": "string" }, "required": false }
            }
        }))
        .assert_value(),
    };

    let schema = contract.provider_schema(ProviderSchemaDialect::OpenAiStrict);
    assert_eq!(
        schema
            .pointer("/properties/response/required")
            .assert_value(),
        &json!(["optional", "required"])
    );
    assert_eq!(
        schema
            .pointer("/properties/response/properties/optional")
            .assert_value(),
        &json!({ "anyOf": [{ "type": "string" }, { "type": "null" }] })
    );
    assert!(matches!(
        resolve_agent_response_with_dialect(
            &contract,
            r#"{"response":{"required":true,"optional":null}}"#,
            ProviderSchemaDialect::OpenAiStrict,
        )
        .assert_value(),
        AgentResponse::Complete(WorkerOutcome::Verified { output, .. })
            if output == json!({"required": true})
    ));
}

#[test]
fn openai_normalizes_optional_nulls_recursively() {
    let contract = NodeResponseContract::Worker {
        output: serde_json::from_value(json!({
            "kind": "record",
            "fields": {
                "items": {
                    "type": {
                        "kind": "array",
                        "items": {
                            "kind": "record",
                            "fields": {
                                "note": { "type": { "kind": "string" }, "required": false }
                            }
                        }
                    },
                    "required": true
                }
            }
        }))
        .assert_value(),
    };

    assert!(matches!(
        resolve_agent_response_with_dialect(
            &contract,
            r#"{"response":{"items":[{"note":null},{"note":"kept"}]}}"#,
            ProviderSchemaDialect::OpenAiStrict,
        )
        .assert_value(),
        AgentResponse::Complete(WorkerOutcome::Verified { output, .. })
            if output == json!({"items": [{}, {"note": "kept"}]})
    ));
}

#[test]
fn openai_distinguishes_optional_null_values_from_omission() {
    let contract = NodeResponseContract::Worker {
        output: serde_json::from_value(json!({
            "kind": "record",
            "fields": {
                "nothing": { "type": { "kind": "null" }, "required": false }
            }
        }))
        .assert_value(),
    };

    let schema = contract.provider_schema(ProviderSchemaDialect::OpenAiStrict);
    assert_eq!(
        schema
            .pointer("/properties/response/properties/nothing")
            .assert_value(),
        &json!({
            "anyOf": [
                { "type": "null" },
                {
                    "type": "string",
                    "enum": [OPENAI_OPTIONAL_NULL_OMISSION],
                    "description": "Use this sentinel when the optional field is omitted."
                }
            ]
        })
    );
    assert!(matches!(
        resolve_agent_response_with_dialect(
            &contract,
            r#"{"response":{"nothing":null}}"#,
            ProviderSchemaDialect::OpenAiStrict,
        )
        .assert_value(),
        AgentResponse::Complete(WorkerOutcome::Verified { output, .. })
            if output == json!({"nothing": null})
    ));

    let omitted = serde_json::to_string(&json!({
        "response": { "nothing": OPENAI_OPTIONAL_NULL_OMISSION }
    }))
    .assert_value();
    assert!(matches!(
        resolve_agent_response_with_dialect(
            &contract,
            &omitted,
            ProviderSchemaDialect::OpenAiStrict,
        )
        .assert_value(),
        AgentResponse::Complete(WorkerOutcome::Verified { output, .. })
            if output == json!({})
    ));
}

#[test]
fn provider_envelope_is_unwrapped_before_authoritative_validation() {
    let contract = worker_contract();
    assert!(matches!(
        resolve_agent_response(&contract, r#"{"response":{"answer":42}}"#).assert_value(),
        AgentResponse::Complete(WorkerOutcome::Verified { output, .. })
            if output == json!({"answer": 42})
    ));
    assert!(matches!(
        resolve_agent_response(&contract, r#"{"answer":42}"#).assert_value(),
        AgentResponse::Correction { .. }
    ));
    assert!(matches!(
        resolve_agent_response(&contract, r#"{"response":{"answer":42},"extra":true}"#)
            .assert_value(),
        AgentResponse::Correction { .. }
    ));
}

#[test]
fn coverage_contract_verifier_response_errors_are_bounded_and_identify_signal_contract_violations()
{
    let verdict = FieldName::new("verdict").assert_value();
    let accepted = EnumLabel::new("accepted").assert_value();
    let contract = NodeResponseContract::Verifier {
        output: PayloadType::Null,
        signals: BTreeMap::from([(
            verdict.clone(),
            NonEmptyEnumSet::new(vec![accepted.clone()]).assert_value(),
        )]),
        diagnostic: PayloadType::String,
    };

    let malformed = contract
        .parse_agent_response(r#"{"output":null}"#)
        .err()
        .assert_value();
    assert!(
        malformed
            .to_string()
            .contains("exactly output, signals, and diagnostic")
    );
    assert!(
        contract
            .validate_agent_outcome(&WorkerOutcome::Verified {
                output: Value::Null,
                artifacts: Vec::new(),
            })
            .is_err()
    );

    let missing = validate_signals(&contract_signals(&contract), &BTreeMap::new())
        .err()
        .assert_value();
    assert!(
        missing
            .to_string()
            .contains("missing required field verdict")
    );
    let extra = validate_signals(
        &contract_signals(&contract),
        &BTreeMap::from([
            (verdict, accepted),
            (
                FieldName::new("future").assert_value(),
                EnumLabel::new("unknown").assert_value(),
            ),
        ]),
    )
    .err()
    .assert_value();
    assert!(extra.to_string().contains("undeclared field future"));

    let bounded = NodeResponseError::new("é".repeat(MAX_RESPONSE_ERROR_BYTES));
    assert!(bounded.to_string().len() <= MAX_RESPONSE_ERROR_BYTES);
    assert!(bounded.to_string().ends_with("..."));
}

fn contract_signals(contract: &NodeResponseContract) -> BTreeMap<FieldName, NonEmptyEnumSet> {
    let NodeResponseContract::Verifier { signals, .. } = contract else {
        unreachable!("test contract is a verifier")
    };
    signals.clone()
}
