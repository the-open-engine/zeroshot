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
    assert!(prompt.contains("rewrite the checkout or shared dependencies"));
    assert!(prompt.contains("other nodes may run concurrently"));
    assert!(prompt.contains("Do not modify reviewed material"));
    assert!(prompt.contains("create artifacts"));
    assert!(prompt.contains("missing prerequisites, incomplete builds and failed checks"));
    assert!(prompt.contains("with command evidence"));
    assert!(prompt.contains("Follow authored instructions when deciding the verdict"));
    assert!(!prompt.contains("rejected"));
    assert!(!prompt.contains("blocked"));
    assert!(prompt.contains(&input.to_string()));
    assert!(prompt.contains(&serde_json::to_string(&verifier).assert_value()));
    let worker = render_agent_prompt(&instructions, &input, &worker_contract()).assert_value();
    assert!(worker.contains("Runtime-owned workspace setup guidance:"));
    assert!(worker.contains("manifest/lockfile dependencies in the checkout"));
    assert!(worker.contains("Wait for setup to finish and check exit status"));
    assert!(worker.contains("Homes are session-local"));
    assert!(
        worker.contains("Prepare required dependencies and candidate binaries in shared paths")
    );
    assert!(worker.contains("report the paths/settings fresh reviewers need"));
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
    let correction = render_agent_correction_with_schema(
        &contract,
        &error,
        None,
        ProviderSchemaDialect::Standard,
    )
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
    // `render_agent_prompt` remains the renderer for Codex and Copilot (including the GitHub
    // lane). Claude selects native tool delivery; the Pi schema renderer is additive. This prompt stays
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
        "Follow repository setup using manifest/lockfile dependencies in the checkout; ",
        "do not use an ad hoc unpinned list. Wait for setup to finish and check exit ",
        "status. Leave ignored dependencies there. Homes are session-local. Prepare required ",
        "dependencies and candidate binaries in shared paths, and report the paths/settings fresh ",
        "reviewers need. Put standalone tools under an executable user path: use ",
        "`$ZEROSHOT_TOOLS/bin` when provided (shared by all nodes), otherwise `$HOME/.local/bin`. ",
        "Do not install tools in `/tmp` (possibly `noexec`).\n",
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

    let missing = validate_signals(&contract_signals(&contract), &BTreeMap::new(), false)
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
        false,
    )
    .err()
    .assert_value();
    assert!(extra.to_string().contains("undeclared field future"));

    let bounded = NodeResponseError::new("é".repeat(MAX_RESPONSE_ERROR_BYTES));
    assert!(bounded.to_string().len() <= MAX_RESPONSE_ERROR_BYTES);
    assert!(bounded.to_string().ends_with("..."));
}

fn review_diagnostic() -> PayloadType {
    serde_json::from_value(json!({
        "kind": "record",
        "fields": {
            "message": { "type": { "kind": "string" }, "required": true },
            "verdict": { "type": { "kind": "string" }, "required": false },
            "findings": {
                "type": {
                    "kind": "array",
                    "items": {
                        "kind": "record",
                        "fields": {
                            "summary": { "type": { "kind": "string" }, "required": true },
                            "location": { "type": { "kind": "string" }, "required": false }
                        }
                    }
                },
                "required": false
            }
        }
    }))
    .assert_value()
}

fn review_response(findings: Value) -> String {
    json!({
        "output": null,
        "signals": {"verdict": "rejected"},
        "diagnostic": {"message": "repair the checkout flow", "findings": findings}
    })
    .to_string()
}

fn finding(summary: String) -> Value {
    json!({"summary": summary, "location": "src/checkout.rs:12"})
}

#[test]
fn an_oversized_review_verdict_is_corrected_on_the_count_then_accepted() {
    let contract = verifier_contract(PayloadType::Null, review_diagnostic());
    let twenty = review_response(Value::Array(
        (0..20)
            .map(|index| finding(format!("finding {index}")))
            .collect(),
    ));
    let correction =
        resolve_agent_response_with_schema(&contract, &twenty, ProviderSchemaDialect::Standard)
            .assert_value();
    let AgentResponse::Correction {
        kind,
        prompt,
        diagnostic,
    } = correction
    else {
        panic!("a 20-finding verdict must request one correction");
    };
    assert_eq!(kind, AgentCorrectionKind::Contract);
    assert!(diagnostic.to_string().contains("has 20 items"));
    assert!(diagnostic.to_string().contains("maximum is 8 findings"));
    assert!(prompt.contains("findings"));
    assert!(prompt.contains("maximum is 8"));

    let eight = review_response(Value::Array(
        (0..8)
            .map(|index| finding(format!("finding {index}")))
            .collect(),
    ));
    assert!(matches!(
        resolve_agent_response_with_schema(&contract, &eight, ProviderSchemaDialect::Standard)
            .assert_value(),
        AgentResponse::Complete(WorkerOutcome::Verifier { .. })
    ));
}

#[test]
fn an_overlong_review_field_is_rejected_by_name() {
    let contract = verifier_contract(PayloadType::Null, review_diagnostic());
    let response = review_response(Value::Array(vec![finding("x".repeat(401))]));
    let correction =
        resolve_agent_response_with_schema(&contract, &response, ProviderSchemaDialect::Standard)
            .assert_value();
    let AgentResponse::Correction { diagnostic, .. } = correction else {
        panic!("an overlong summary must request a correction");
    };
    assert!(diagnostic.to_string().contains("findings[0].summary"));
    assert!(diagnostic.to_string().contains("maximum is 400"));

    let long_verdict = json!({
        "output": null,
        "signals": {"verdict": "rejected"},
        "diagnostic": {"message": "repair", "verdict": "v".repeat(65)}
    })
    .to_string();
    let correction = resolve_agent_response_with_schema(
        &contract,
        &long_verdict,
        ProviderSchemaDialect::Standard,
    )
    .assert_value();
    let AgentResponse::Correction { diagnostic, .. } = correction else {
        panic!("an overlong verdict must request a correction");
    };
    assert!(diagnostic.to_string().contains("diagnostic.verdict"));
}

#[test]
fn a_review_schema_states_the_bounds_and_output_budget() {
    let contract = verifier_contract(PayloadType::Null, review_diagnostic());
    let schema = contract.provider_schema(ProviderSchemaDialect::Standard);
    let findings =
        &schema["properties"]["response"]["properties"]["diagnostic"]["properties"]["findings"];
    assert_eq!(findings["maxItems"], 8);
    assert_eq!(findings["items"]["properties"]["summary"]["maxLength"], 400);
    let instructions = NodeInstructions::new("Review the change.").assert_value();
    let prompt = render_agent_prompt_with_schema(
        &instructions,
        &Value::Null,
        &contract,
        ProviderSchemaDialect::Standard,
    )
    .assert_value();
    assert!(prompt.contains("Review output budget"));
    assert!(prompt.contains("At most 8 findings"));
    assert!(prompt.contains("at most 400 characters"));

    let strict = contract.provider_schema(ProviderSchemaDialect::OpenAiStrict);
    let strict_json = serde_json::to_string(&strict).assert_value();
    assert!(!strict_json.contains("maxItems"));
    assert!(!strict_json.contains("maxLength"));
    let strict_signals = &strict["properties"]["response"]["properties"]["signals"];
    assert!(strict_signals["properties"].get("truncated").is_none());
    assert!(
        !strict_signals["required"]
            .as_array()
            .assert_value()
            .iter()
            .any(|name| name == "truncated")
    );

    let limited = render_agent_prompt_with_schema_and_budget(AgentPromptRequest {
        instructions: &instructions,
        input: &Value::Null,
        response: &contract,
        dialect: ProviderSchemaDialect::Standard,
        max_output_tokens: Some(512),
    })
    .assert_value();
    assert!(limited.contains("Review output budget: at most 512 tokens"));
}

#[test]
fn a_truncated_review_builds_a_valid_truncated_signal_outcome() {
    let mut contract = verifier_contract(PayloadType::Null, review_diagnostic());
    let NodeResponseContract::Verifier { signals, .. } = &mut contract else {
        unreachable!("test contract is a verifier")
    };
    signals.insert(
        FieldName::new("truncated").assert_value(),
        NonEmptyEnumSet::new(vec![
            EnumLabel::new("true").assert_value(),
            EnumLabel::new("false").assert_value(),
        ])
        .assert_value(),
    );
    let outcome = contract
        .truncated_outcome("Pi stopped at its output limit after receiving 1296 bytes")
        .assert_value();
    let WorkerOutcome::Verifier {
        output,
        signals,
        diagnostic,
        ..
    } = outcome
    else {
        panic!("a review verifier must build a verifier outcome");
    };
    assert_eq!(output, Value::Null);
    assert_eq!(
        signals
            .get(&FieldName::new("truncated").assert_value())
            .assert_value()
            .as_str(),
        "true"
    );
    assert_eq!(
        signals
            .get(&FieldName::new("verdict").assert_value())
            .assert_value()
            .as_str(),
        "rejected"
    );
    assert!(
        diagnostic["message"]
            .as_str()
            .assert_value()
            .contains("1296 bytes")
    );
}

fn review_contract_with_truncated_signal() -> NodeResponseContract {
    let mut contract = verifier_contract(PayloadType::Null, review_diagnostic());
    let NodeResponseContract::Verifier { signals, .. } = &mut contract else {
        unreachable!("test contract is a verifier")
    };
    signals.insert(
        FieldName::new("truncated").assert_value(),
        NonEmptyEnumSet::new(vec![
            EnumLabel::new("true").assert_value(),
            EnumLabel::new("false").assert_value(),
        ])
        .assert_value(),
    );
    contract
}

#[test]
fn an_ordinary_review_verdict_does_not_need_the_optional_truncated_signal() {
    let contract = review_contract_with_truncated_signal();
    let response = json!({
        "output": null,
        "signals": {"verdict": "accepted"},
        "diagnostic": {"message": "the change meets the request"}
    })
    .to_string();
    assert!(matches!(
        resolve_agent_response_with_schema(&contract, &response, ProviderSchemaDialect::Standard)
            .assert_value(),
        AgentResponse::Complete(WorkerOutcome::Verifier { .. })
    ));
    let schema = contract.provider_schema(ProviderSchemaDialect::Standard);
    let required = schema["properties"]["response"]["properties"]["signals"]["required"]
        .as_array()
        .assert_value();
    assert!(!required.iter().any(|name| name == "truncated"));
}

#[test]
fn a_message_and_findings_over_the_combined_cap_are_rejected_by_name() {
    let contract = verifier_contract(PayloadType::Null, review_diagnostic());
    let response = json!({
        "output": null,
        "signals": {"verdict": "rejected"},
        "diagnostic": {
            "message": "m".repeat(4_000),
            "findings": (0..8)
                .map(|index| json!({
                    "summary": format!("{index}{}", "s".repeat(399)),
                    "location": "src/checkout.rs:12"
                }))
                .collect::<Vec<_>>()
        }
    })
    .to_string();
    let AgentResponse::Correction { diagnostic, .. } =
        resolve_agent_response_with_schema(&contract, &response, ProviderSchemaDialect::Standard)
            .assert_value()
    else {
        panic!("message plus findings over the cap must request a correction");
    };
    assert!(
        diagnostic
            .to_string()
            .contains("diagnostic.message plus findings")
    );
    assert!(diagnostic.to_string().contains("maximum is 4000"));
}

#[test]
fn an_overlong_review_message_is_rejected_by_name() {
    let contract = verifier_contract(PayloadType::Null, review_diagnostic());
    let response = json!({
        "output": null,
        "signals": {"verdict": "rejected"},
        "diagnostic": {"message": "x".repeat(4_001)}
    })
    .to_string();
    let correction =
        resolve_agent_response_with_schema(&contract, &response, ProviderSchemaDialect::Standard)
            .assert_value();
    let AgentResponse::Correction { diagnostic, .. } = correction else {
        panic!("an overlong message must request a correction");
    };
    assert!(diagnostic.to_string().contains("diagnostic.message"));
    assert!(diagnostic.to_string().contains("maximum is 4000"));
}

#[test]
fn structured_findings_are_merged_into_the_feedback_message() {
    let contract = verifier_contract(PayloadType::Null, review_diagnostic());
    let response = json!({
        "output": null,
        "signals": {"verdict": "rejected"},
        "diagnostic": {
            "message": "repair the checkout flow",
            "findings": [{"summary": "missing empty-cart check", "location": "src/checkout.rs:12"}]
        }
    })
    .to_string();
    let AgentResponse::Complete(WorkerOutcome::Verifier { diagnostic, .. }) =
        resolve_agent_response_with_schema(&contract, &response, ProviderSchemaDialect::Standard)
            .assert_value()
    else {
        panic!("an ordinary verdict must parse");
    };
    let message = diagnostic["message"].as_str().assert_value();
    assert!(message.contains("repair the checkout flow"), "{message}");
    assert!(message.contains("missing empty-cart check"), "{message}");
    assert!(message.contains("src/checkout.rs:12"), "{message}");
}

#[test]
fn an_ordinary_good_review_verdict_still_parses_unchanged() {
    let contract = verifier_contract(PayloadType::Null, review_diagnostic());
    let response = review_response(Value::Array(vec![finding("one issue".to_owned())]));
    let AgentResponse::Complete(WorkerOutcome::Verifier {
        signals,
        diagnostic,
        ..
    }) = resolve_agent_response_with_schema(&contract, &response, ProviderSchemaDialect::Standard)
        .assert_value()
    else {
        panic!("an ordinary verdict must still parse");
    };
    assert_eq!(
        signals
            .get(&FieldName::new("verdict").assert_value())
            .assert_value()
            .as_str(),
        "rejected"
    );
    assert_eq!(diagnostic["findings"][0]["summary"], "one issue");
}

fn contract_signals(contract: &NodeResponseContract) -> BTreeMap<FieldName, NonEmptyEnumSet> {
    let NodeResponseContract::Verifier { signals, .. } = contract else {
        unreachable!("test contract is a verifier")
    };
    signals.clone()
}

const VERIFIER_SKELETON: &str = concat!(
    r#"{"response":{"output":{"note"?:<integer>,"summary":<string>},"#,
    r#""signals":{"verdict":<one of "approved", "rejected">},"diagnostic":<string>}}"#
);

const STRICT_VERIFIER_SKELETON: &str = concat!(
    r#"{"response":{"output":{"note":<integer> or null,"summary":<string>},"#,
    r#""signals":{"verdict":<one of "approved", "rejected">},"diagnostic":<string>}}"#
);

fn verifier_contract() -> NodeResponseContract {
    NodeResponseContract::Verifier {
        output: serde_json::from_value(json!({
            "kind": "record",
            "fields": {
                "summary": { "type": { "kind": "string" }, "required": true },
                "note": { "type": { "kind": "integer" }, "required": false }
            }
        }))
        .assert_value(),
        signals: BTreeMap::from([(
            FieldName::new("verdict").assert_value(),
            NonEmptyEnumSet::new(vec![
                EnumLabel::new("approved").assert_value(),
                EnumLabel::new("rejected").assert_value(),
            ])
            .assert_value(),
        )]),
        diagnostic: serde_json::from_value(json!({ "kind": "string" })).assert_value(),
    }
}

fn valid_verifier_envelope() -> String {
    json!({
        "response": {
            "output": { "summary": "looks good" },
            "signals": { "verdict": "approved" },
            "diagnostic": "checked"
        }
    })
    .to_string()
}

fn correction_prompt(contract: &NodeResponseContract, response: &str) -> String {
    match resolve_agent_response_with_schema(contract, response, ProviderSchemaDialect::Standard)
        .assert_value()
    {
        AgentResponse::Correction { prompt, .. } => prompt,
        AgentResponse::Complete(_) => panic!("rejected response must request a correction"),
    }
}

#[test]
fn response_skeleton_renders_the_verifier_envelope() {
    assert_eq!(
        response_skeleton(&verifier_contract(), ProviderSchemaDialect::Standard).assert_value(),
        VERIFIER_SKELETON
    );
}

#[test]
fn response_skeleton_renders_every_payload_kind() {
    let contract = NodeResponseContract::Worker {
        output: serde_json::from_value(json!({
            "kind": "record",
            "fields": {
                "nothing": { "type": { "kind": "null" }, "required": true },
                "flag": { "type": { "kind": "boolean" }, "required": true },
                "count": { "type": { "kind": "integer" }, "required": true },
                "ratio": { "type": { "kind": "number" }, "required": true },
                "text": { "type": { "kind": "string" }, "required": true },
                "names": {
                    "type": {
                        "kind": "array",
                        "items": { "kind": "string" }
                    },
                    "required": true
                },
                "state": {
                    "type": { "kind": "enum", "values": ["fresh", "ripe"] },
                    "required": true
                },
                "nested": {
                    "type": {
                        "kind": "record",
                        "fields": {
                            "note": { "type": { "kind": "string" }, "required": false }
                        }
                    },
                    "required": true
                }
            }
        }))
        .assert_value(),
    };
    assert_eq!(
        response_skeleton(&contract, ProviderSchemaDialect::Standard).assert_value(),
        concat!(
            r#"{"response":{"count":<integer>,"flag":<boolean>,"names":[<string>,...],"#,
            r#""nested":{"note"?:<string>},"nothing":null,"ratio":<number>,"#,
            r#""state":<one of "fresh", "ripe">,"text":<string>}}"#
        )
    );
    assert_eq!(
        response_skeleton(&worker_contract(), ProviderSchemaDialect::Standard).assert_value(),
        r#"{"response":{"answer":<integer>}}"#
    );
}

#[test]
fn strict_skeleton_writes_every_optional_field_with_its_omission_value() {
    assert_eq!(
        response_skeleton(&verifier_contract(), ProviderSchemaDialect::OpenAiStrict).assert_value(),
        STRICT_VERIFIER_SKELETON
    );
}

#[test]
fn strict_skeleton_uses_the_sentinel_for_optional_null_fields() {
    let contract = NodeResponseContract::Worker {
        output: serde_json::from_value(json!({
            "kind": "record",
            "fields": {
                "gap": { "type": { "kind": "null" }, "required": false },
                "n": { "type": { "kind": "integer" }, "required": true }
            }
        }))
        .assert_value(),
    };
    assert_eq!(
        response_skeleton(&contract, ProviderSchemaDialect::OpenAiStrict).assert_value(),
        r#"{"response":{"gap":null or "__zeroshot_omitted_optional_null__","n":<integer>}}"#
    );
    assert_eq!(
        response_skeleton(&contract, ProviderSchemaDialect::Standard).assert_value(),
        r#"{"response":{"gap"?:null,"n":<integer>}}"#
    );
}

#[test]
fn strict_schema_correction_shows_the_strict_shape() {
    let response = format!("Here is my review:\n{}", valid_verifier_envelope());
    let AgentResponse::Correction { prompt, .. } = resolve_agent_response_with_schema(
        &verifier_contract(),
        &response,
        ProviderSchemaDialect::OpenAiStrict,
    )
    .assert_value() else {
        panic!("rejected response must request a correction");
    };
    assert!(prompt.contains(STRICT_VERIFIER_SKELETON));
    assert!(prompt.contains(STRICT_EXPECTED_SHAPE_HEADER));
    assert!(!prompt.contains("\"note\"?"));
}

#[test]
fn contract_correction_keeps_the_standard_shape_for_strict_lanes() {
    let response = format!("Here is my review:\n{}", valid_verifier_envelope());
    let AgentResponse::Correction { prompt, .. } = resolve_agent_response_with_dialect(
        &verifier_contract(),
        &response,
        ProviderSchemaDialect::OpenAiStrict,
    )
    .assert_value() else {
        panic!("rejected response must request a correction");
    };
    assert!(prompt.contains("\"note\"?:<integer>"));
    assert!(prompt.contains(EXPECTED_SHAPE_HEADER));
}

#[test]
fn skeleton_with_placeholders_is_not_accepted() {
    let skeleton =
        response_skeleton(&verifier_contract(), ProviderSchemaDialect::Standard).assert_value();
    assert!(serde_json::from_str::<Value>(&skeleton).is_err());
    assert!(matches!(
        resolve_agent_response_with_schema(
            &verifier_contract(),
            &skeleton,
            ProviderSchemaDialect::Standard
        )
        .assert_value(),
        AgentResponse::Correction { .. }
    ));
}

#[test]
fn correction_names_text_before_the_object() {
    let response = format!("Here is my review:\n{}", valid_verifier_envelope());
    let prompt = correction_prompt(&verifier_contract(), &response);
    assert!(prompt.contains("Problem:"));
    assert!(prompt.contains(EnvelopeFault::LeadingText.problem()));
    assert!(prompt.contains(VERIFIER_SKELETON));
}

#[test]
fn correction_names_an_unterminated_object() {
    let envelope = valid_verifier_envelope();
    let truncated = envelope.strip_suffix('}').assert_value();
    let prompt = correction_prompt(&verifier_contract(), truncated);
    assert!(prompt.contains("Problem:"));
    assert!(prompt.contains(EnvelopeFault::Unterminated.problem()));
}

#[test]
fn correction_names_content_after_the_object() {
    let envelope = valid_verifier_envelope();
    let prompt = correction_prompt(&verifier_contract(), &format!("{envelope}{envelope}"));
    assert!(prompt.contains("Problem:"));
    assert!(prompt.contains(EnvelopeFault::TrailingContent.problem()));
}

#[test]
fn correction_names_an_empty_reply() {
    let prompt = correction_prompt(&verifier_contract(), "  \n");
    assert!(prompt.contains("Problem:"));
    assert!(prompt.contains(EnvelopeFault::Empty.problem()));
}

#[test]
fn payload_errors_carry_the_shape_but_no_envelope_problem() {
    let response = json!({
        "response": {
            "output": { "summary": "looks good" },
            "signals": { "verdict": "maybe" },
            "diagnostic": "checked"
        }
    })
    .to_string();
    let prompt = correction_prompt(&verifier_contract(), &response);
    assert!(prompt.contains(VERIFIER_SKELETON));
    assert!(!prompt.contains("Problem:"));
}

#[test]
fn correction_guidance_stays_out_of_the_diagnostic() {
    let response = format!("Here is my review:\n{}", valid_verifier_envelope());
    let AgentResponse::Correction { diagnostic, .. } = resolve_agent_response_with_schema(
        &verifier_contract(),
        &response,
        ProviderSchemaDialect::Standard,
    )
    .assert_value() else {
        panic!("rejected response must request a correction");
    };
    assert!(
        diagnostic
            .to_string()
            .starts_with("provider response must be exactly an object containing response:")
    );
    assert!(!diagnostic.to_string().contains("Problem:"));
    assert!(!diagnostic.to_string().contains("Expected shape"));
}

#[test]
fn contract_correction_carries_the_same_guidance() {
    let response = format!("Here is my review:\n{}", valid_verifier_envelope());
    let AgentResponse::Correction { prompt, .. } =
        resolve_agent_response(&verifier_contract(), &response).assert_value()
    else {
        panic!("rejected response must request a correction");
    };
    assert!(prompt.contains("Response contract:"));
    assert!(prompt.contains("Problem:"));
    assert!(prompt.contains(VERIFIER_SKELETON));
}

#[test]
fn envelope_fault_classifies_known_faults() {
    assert_eq!(envelope_fault(""), Some(EnvelopeFault::Empty));
    assert_eq!(envelope_fault("ok {}"), Some(EnvelopeFault::LeadingText));
    assert_eq!(
        envelope_fault("{\"response\":{"),
        Some(EnvelopeFault::Unterminated)
    );
    assert_eq!(
        envelope_fault("{} {}"),
        Some(EnvelopeFault::TrailingContent)
    );
    assert_eq!(envelope_fault("{\"other\":1}"), None);
}

#[test]
fn tool_delivery_preserves_worker_null_and_verifier_contracts() {
    let instructions = NodeInstructions::new("Complete the task.").assert_value();
    let input = json!({"task":"review"});
    let cases = [
        (worker_contract(), json!({"answer":42})),
        (
            NodeResponseContract::Worker {
                output: PayloadType::Null,
            },
            Value::Null,
        ),
        (
            verifier_contract(),
            json!({
                "output":{"summary":"checked"},
                "signals":{"verdict":"approved"},
                "diagnostic":"checked"
            }),
        ),
    ];
    for (contract, valid) in cases {
        let message = render_agent_prompt(&instructions, &input, &contract).assert_value();
        let tool = render_agent_prompt_with_delivery(
            &instructions,
            &input,
            &contract,
            ResponseDelivery::StructuredOutput,
            None,
        )
        .assert_value();
        // Authored instructions, runtime guidance, input and the contract precede delivery wording.
        let boundary = "The response contract describes the required type;";
        assert_eq!(
            message.split_once(boundary).assert_value().0,
            tool.split_once(boundary).assert_value().0
        );
        assert!(tool.contains("using the session-provided StructuredOutput tool"));
        assert!(tool.contains("exactly once"));
        assert!(tool.contains("Do not also write the final answer as a text message"));
        assert!(!tool.contains("Return only JSON"));
        assert!(tool.contains("one field named response"));
        assert!(tool.contains("For a verifier, response contains exactly an object with output"));
        let contract_json = serde_json::to_string(&contract).assert_value();
        assert!(tool.contains(&contract_json));
        for invalid in ["", "prefix {}", "{", "{} {}", r#"{"response":false}"#] {
            let AgentResponse::Correction { prompt, diagnostic } =
                resolve_agent_response_with_delivery(
                    &contract,
                    invalid,
                    ResponseDelivery::StructuredOutput,
                )
                .assert_value()
            else {
                panic!("invalid response must be corrected");
            };
            let AgentResponse::Correction {
                diagnostic: original,
                ..
            } = resolve_agent_response(&contract, invalid).assert_value()
            else {
                panic!("invalid response must be corrected");
            };
            assert_eq!(diagnostic.to_string(), original.to_string());
            assert!(prompt.contains(&contract_json));
            assert!(prompt.contains("using the session-provided StructuredOutput tool"));
            assert!(prompt.contains("exactly once"));
            assert!(prompt.contains("Do not also write the final answer as a text message"));
            assert!(!prompt.contains("Start the reply"));
            assert!(!prompt.contains("Return a corrected provider response only"));
            assert!(!prompt.contains("Expected shape"));
            assert!(!prompt.contains("The reply began with text"));
        }
        let envelope = json!({"response":valid}).to_string();
        let AgentResponse::Complete(actual) = resolve_agent_response_with_delivery(
            &contract,
            &envelope,
            ResponseDelivery::StructuredOutput,
        )
        .assert_value() else {
            panic!("valid response must complete");
        };
        let AgentResponse::Complete(expected) =
            resolve_agent_response(&contract, &envelope).assert_value()
        else {
            panic!("valid response must complete");
        };
        assert_eq!(actual, expected);
        // Selecting Claude's delivery never changes subsequent text-lane invocations.
        assert_eq!(
            render_agent_prompt(&instructions, &input, &contract).assert_value(),
            message
        );
    }
}
