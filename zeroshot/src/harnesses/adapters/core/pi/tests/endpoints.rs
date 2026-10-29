//! Caller-owned endpoint tests for the Pi lanes.
//!
//! The `openai` lane honors its own endpoint variables, and the `anthropic` lane converts a
//! declared or ambient `ANTHROPIC_BASE_URL` into a provider override. These tests drive the
//! adapter itself (not the pure `command` helper), so the provider-document decision and the
//! private agent directory it requires are exercised together.
//!
//! A child module of `native_v2_pi/tests.rs`, so the shared fixtures stay in one place and the
//! coverage totals keep excluding this file (`AGENTS.md`).

use super::*;

fn local_adapter(provider: PiProvider, native_environment: BTreeMap<String, String>) -> PiAdapter {
    PiAdapter::new_local(adapter_configuration(provider, "pi", native_environment)).assert_value()
}

/// The provider document the adapter wrote into the private agent directory.
fn written_models(files: &ProviderExecutionFiles) -> String {
    std::fs::read_to_string(files.home().join(MODELS_FILE))
        .assert_value_with("a caller-owned endpoint writes the provider document Pi reads")
}

fn endpoint_command(
    adapter: &PiAdapter,
    files: &ProviderExecutionFiles,
    fields: &[&str],
    values: &[(&str, &str)],
) -> Result<
    crate::execution::process::ProcessSessionCommand,
    crate::native_v2_runner::NodeRunnerError,
> {
    adapter.command_for_test(
        &invocation(
            agent_binding("gpt-5.6", None, SessionScope::Execution, fields),
            NodeRole::Worker,
            resolved(fields, values),
        ),
        files,
        "zs-fixed",
    )
}

#[test]
fn coverage_contract_pi_openai_lane_turns_a_declared_base_url_into_a_provider_override() {
    let directory = TestDirectory::new("pi-openai-declared-base-url");
    let files = files(&directory);
    let adapter = local_adapter(PiProvider::OpenAi, BTreeMap::new());
    let fields = ["OPENAI_API_KEY", "OPENAI_BASE_URL"];
    let command = endpoint_command(
        &adapter,
        &files,
        &fields,
        &[
            ("OPENAI_API_KEY", "declared"),
            ("OPENAI_BASE_URL", "https://openai-proxy.example/v1"),
        ],
    )
    .assert_value_with("declared OpenAI endpoint");
    assert_eq!(
        value(&command.argv, "--provider").as_deref(),
        Some("openai")
    );
    // A caller-owned endpoint ends native login reuse and writes the private document Pi reads.
    assert_eq!(
        command
            .environment
            .get("PI_CODING_AGENT_DIR")
            .map(String::as_str),
        Some(files.home().to_string_lossy().as_ref())
    );
    let document = written_models(&files);
    assert!(
        document.contains("https://openai-proxy.example/v1"),
        "{document}"
    );
    assert!(document.contains("$OPENAI_API_KEY"), "{document}");
    // The declared secret is referenced, never written into the document.
    assert!(!document.contains("declared"), "{document}");
}

#[test]
fn coverage_contract_pi_openai_lane_falls_back_to_the_compatible_api_base_variable() {
    let directory = TestDirectory::new("pi-openai-api-base");
    let files = files(&directory);
    let adapter = local_adapter(PiProvider::OpenAi, BTreeMap::new());
    let fields = ["OPENAI_API_KEY", "OPENAI_API_BASE"];
    endpoint_command(
        &adapter,
        &files,
        &fields,
        &[
            ("OPENAI_API_KEY", "declared"),
            ("OPENAI_API_BASE", "https://api-base.example/v1"),
        ],
    )
    .assert_value_with("declared OpenAI compatible endpoint");
    assert!(
        written_models(&files).contains("https://api-base.example/v1"),
        "the compatible API base variable must retarget the lane"
    );
}

#[test]
fn coverage_contract_pi_openai_lane_honors_an_ambient_base_url() {
    // The OpenAI lane inherits the same endpoint variables Codex does, so a local user's exported
    // proxy must retarget Pi rather than being forwarded as a variable Pi ignores.
    let directory = TestDirectory::new("pi-openai-ambient-base");
    let files = files(&directory);
    let adapter = local_adapter(
        PiProvider::OpenAi,
        BTreeMap::from([
            ("OPENAI_API_KEY".to_owned(), "ambient".to_owned()),
            (
                "OPENAI_BASE_URL".to_owned(),
                "https://ambient-proxy.example/v1".to_owned(),
            ),
        ]),
    );
    let command =
        endpoint_command(&adapter, &files, &[], &[]).assert_value_with("ambient OpenAI endpoint");
    assert_eq!(
        command
            .environment
            .get("OPENAI_API_KEY")
            .map(String::as_str),
        Some("ambient")
    );
    assert!(
        written_models(&files).contains("https://ambient-proxy.example/v1"),
        "an ambient endpoint must become a provider override"
    );
}

#[test]
fn coverage_contract_pi_openai_lane_honors_an_ambient_api_base_when_the_base_url_is_absent() {
    let directory = TestDirectory::new("pi-openai-ambient-api-base");
    let files = files(&directory);
    let adapter = local_adapter(
        PiProvider::OpenAi,
        BTreeMap::from([
            ("OPENAI_API_KEY".to_owned(), "ambient".to_owned()),
            (
                "OPENAI_API_BASE".to_owned(),
                "https://ambient-api-base.example/v1".to_owned(),
            ),
        ]),
    );
    endpoint_command(&adapter, &files, &[], &[])
        .assert_value_with("ambient OpenAI compatible endpoint");
    assert!(
        written_models(&files).contains("https://ambient-api-base.example/v1"),
        "the compatible API base variable must retarget the lane"
    );
}

#[test]
fn coverage_contract_pi_a_caller_owned_openai_endpoint_still_requires_its_lane_credential() {
    // A declared endpoint ends native login reuse, so the run must carry the lane's own credential
    // instead of silently reusing a stored one against a caller-owned host.
    let directory = TestDirectory::new("pi-openai-endpoint-no-key");
    let files = files(&directory);
    let adapter = local_adapter(PiProvider::OpenAi, BTreeMap::new());
    let fields = ["OPENAI_BASE_URL"];
    let outcome = endpoint_command(
        &adapter,
        &files,
        &fields,
        &[("OPENAI_BASE_URL", "https://openai-proxy.example/v1")],
    );
    assert!(
        outcome.is_err(),
        "a caller-owned endpoint without the lane credential must fail closed"
    );
}

#[test]
fn coverage_contract_pi_bedrock_lane_writes_no_endpoint_document() {
    // Bedrock has no caller-owned endpoint override, so the adapter must leave Pi's own
    // configuration untouched while still using a private agent directory.
    let directory = TestDirectory::new("pi-bedrock-no-endpoint");
    let files = files(&directory);
    let adapter = local_adapter(PiProvider::Bedrock, BTreeMap::new());
    let fields = ["AWS_BEARER_TOKEN_BEDROCK", "AWS_REGION"];
    let command = endpoint_command(
        &adapter,
        &files,
        &fields,
        &[
            ("AWS_BEARER_TOKEN_BEDROCK", "bedrock-token"),
            ("AWS_REGION", "us-east-1"),
        ],
    )
    .assert_value_with("bedrock launch");
    assert_eq!(
        value(&command.argv, "--provider").as_deref(),
        Some("amazon-bedrock")
    );
    assert!(command.environment.contains_key("PI_CODING_AGENT_DIR"));
    assert!(!files.home().join(MODELS_FILE).exists());
}

#[test]
fn coverage_contract_pi_openrouter_lane_writes_no_endpoint_document() {
    let directory = TestDirectory::new("pi-openrouter-no-endpoint");
    let files = files(&directory);
    let adapter = local_adapter(PiProvider::OpenRouter, BTreeMap::new());
    let fields = ["OPENROUTER_API_KEY"];
    let command = endpoint_command(
        &adapter,
        &files,
        &fields,
        &[("OPENROUTER_API_KEY", "openrouter-key")],
    )
    .assert_value_with("openrouter launch");
    assert_eq!(
        value(&command.argv, "--provider").as_deref(),
        Some("openrouter")
    );
    assert!(!files.home().join(MODELS_FILE).exists());
}

#[test]
fn coverage_contract_pi_gateway_requires_a_complete_connection_before_launch() {
    // A gateway lane with only its base URL cannot resolve a connection, so the adapter must fail
    // before writing a document or starting a child.
    let directory = TestDirectory::new("pi-gateway-incomplete");
    let files = files(&directory);
    let adapter = local_adapter(PiProvider::Gateway, BTreeMap::new());
    let fields = ["GATEWAY_BASE_URL"];
    let outcome = endpoint_command(
        &adapter,
        &files,
        &fields,
        &[("GATEWAY_BASE_URL", "https://gateway.example/v1")],
    );
    assert!(
        outcome.is_err(),
        "an incomplete gateway connection must fail"
    );
}

#[test]
fn coverage_contract_pi_endpoint_launch_requires_an_agent_binding() {
    let directory = TestDirectory::new("pi-endpoint-delivery-binding");
    let files = files(&directory);
    let adapter = local_adapter(PiProvider::Anthropic, BTreeMap::new());
    let outcome = adapter.command_for_test(
        &invocation(delivery_binding(), NodeRole::Worker, resolved(&[], &[])),
        &files,
        "zs-fixed",
    );
    assert!(outcome.is_err(), "a delivery binding is not an agent turn");
}
