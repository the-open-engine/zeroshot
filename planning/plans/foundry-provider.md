# Microsoft Foundry provider lane

Status: approved with the proposed defaults on 2026-11-02; Zeroshot PR in progress. See
[Decisions](#decisions).

## Problem

Teams on Azure reach Claude and OpenAI models through Microsoft Foundry. Zeroshot has no provider
for it. Today the only route is `provider: "gateway"`:

- Codex/gateway sends the key as a bearer token to a generic Responses endpoint. It works against
  Azure OpenAI, but it is undocumented and does not use Codex's own Azure provider shape.
- Claude/gateway sends the key in `x-api-key` to a generic Messages endpoint. Whether Foundry's
  Anthropic endpoint accepts that is unverified.
- Claude Code's native Foundry mode is half-wired: `CLAUDE_CODE_USE_FOUNDRY` is in the Claude
  local environment allowlist and is rejected on non-Anthropic lanes, but the credential and
  resource variables it needs (`ANTHROPIC_FOUNDRY_API_KEY`, `ANTHROPIC_FOUNDRY_RESOURCE`,
  `ANTHROPIC_FOUNDRY_BASE_URL`) are not passed through, so it cannot work locally or hosted.

Amazon Bedrock is the precedent: the same kind of cloud-hosted model access, shipped as a
first-class `bedrock` lane for Codex and Claude in one change (#1078).

## Goal

Add a `foundry` provider for the `codex` and `claude` harnesses, authenticated with a Foundry API
key, usable locally and on Zeroshot Platform, with the same admission, conflict, and secrecy
guarantees as `bedrock`.

Out of scope for this change:

- Entra ID / managed identity authentication (`ANTHROPIC_FOUNDRY_AUTH_TOKEN`, Azure CLI login).
- Foundry lanes for Pi, Copilot, OpenCode, and Hermes. Their registrations do not list `foundry`,
  so admission rejects those lanes. Copilot's existing Azure setting on its `openai` lane is
  unchanged.
- Non-Claude, non-OpenAI models hosted in Foundry.

## Design

### Connection

One connection serves both harnesses, so a run with a Codex worker and Claude reviewers on the same
Foundry resource needs one stored connection, as with Bedrock.

| Provider  | Connection key | Fields                                            |
| --------- | -------------- | ------------------------------------------------- |
| `foundry` | `foundry`      | `AZURE_FOUNDRY_API_KEY`, `AZURE_FOUNDRY_RESOURCE` |

Alternative set: `AZURE_FOUNDRY_API_KEY`, `AZURE_FOUNDRY_BASE_URL`, for custom domains and private
endpoints. Exactly one of `AZURE_FOUNDRY_RESOURCE` and `AZURE_FOUNDRY_BASE_URL` must be present.
The resource name is validated as an Azure resource name (lowercase letters, digits, hyphens); the
base URL follows the existing gateway URL rules (HTTP(S), no credentials, query, or fragment).

The model is the Foundry deployment name, passed unchanged as the opaque model identifier.

### Claude

Mirror `configure_bedrock` in `zeroshot/src/harnesses/adapters/core/claude/command.rs`:

- Require the key and one endpoint field; reject `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`,
  Claude OAuth tokens, `OPENROUTER_API_KEY`, and `AWS_BEARER_TOKEN_BEDROCK`.
- Translate to Claude Code's native names: `ANTHROPIC_FOUNDRY_API_KEY`, and either
  `ANTHROPIC_FOUNDRY_RESOURCE` or `ANTHROPIC_FOUNDRY_BASE_URL`; set `CLAUDE_CODE_USE_FOUNDRY=1`.
  Remove the `AZURE_FOUNDRY_*` names from the process environment.
- In `validate_provider_selection`, `CLAUDE_CODE_USE_FOUNDRY` is compatible only with the Foundry
  lane; every other active selector conflicts.
- `access.rs`: add the `foundry` access contract (not natively authenticated).
- `registration.rs`: add `"foundry"` to providers.

Separate small fix, same PR or its own: add `ANTHROPIC_FOUNDRY_API_KEY`,
`ANTHROPIC_FOUNDRY_RESOURCE`, and `ANTHROPIC_FOUNDRY_BASE_URL` to the Claude
`LOCAL_ENVIRONMENT` allowlist so a local `claude/anthropic` run started from a shell already in
Foundry mode keeps working, matching how `ANTHROPIC_BEDROCK_BASE_URL` is handled.

### Codex

Codex has no built-in Azure provider; its documented Azure setup is a custom `model_providers`
entry with `wire_api = "responses"`. In `zeroshot/src/harnesses/adapters/core/codex/command.rs`:

- `configure_provider_auth`: require the key and one endpoint field; reject `CODEX_API_KEY`,
  `OPENAI_API_KEY`, `OPENROUTER_API_KEY`, and `AWS_BEARER_TOKEN_BEDROCK`.
- `add_provider_args`: emit `model_provider="foundry"` and
  - `model_providers.foundry.base_url` = `https://{resource}.openai.azure.com/openai/v1`, or the
    supplied root URL followed by `/openai/v1`;
  - `model_providers.foundry.wire_api="responses"`;
  - `model_providers.foundry.requires_openai_auth=false`;
  - `model_providers.foundry.env_http_headers={ "api-key" = "AZURE_FOUNDRY_API_KEY" }`, so the key
    goes in Azure's documented header rather than a bearer token.
- `access.rs`, `registration.rs`, and the `CodexProvider` enum as for Claude.

The v1 Azure OpenAI API needs no `api-version` query parameter. If the live test shows a deployment
still needs one, add `query_params` rather than a new connection field.

### Protocol, CLI, docs

- `crates/openengine-cluster-protocol/src/native_v2_run.rs`: add `Foundry` to `ClaudeProvider`
  and `CodexProvider` and to `native_provider_names!`; regenerate
  `protocol/openengine-cluster/v1/schema.json` through the Rust testkit and confirm with
  `npm run protocol:check`.
- `zeroshot/src/native_v2_cli/parser.rs` help text; regenerate `docs/zeroshot-cli.{md,html}` with
  the Rust example (never hand-edited).
- `docs/concepts/runtimes-and-connections.md`: add the table row and a short Foundry section next
  to Gateways; `docs/project/harness-adapters.md` and `AGENTS.md` where Bedrock is described.
- Python SDK: check `sdks/python` for provider literals and add `foundry` if they are enumerated.

### Zeroshot Platform (separate change)

The hosted path needs no runtime change: connection values arrive as opaque fields and Zeroshot
interprets them in the capsule. The Platform console needs a `foundry` connection with key and
resource fields, a connection check, and matching documentation. That change lands after the
Zeroshot release that adds the lane.

## Tests

Follow the Bedrock coverage in #1078:

- Protocol: serialization round-trip and schema for `foundry` on both harnesses.
- Admission: canonical requirements materialize; Pi/Copilot/OpenCode/Hermes `foundry` lanes are
  rejected; a mixed Codex+Claude run requires one `foundry` connection.
- Claude environment: translation to `ANTHROPIC_FOUNDRY_*`, `CLAUDE_CODE_USE_FOUNDRY=1`, removal
  of `AZURE_FOUNDRY_*`, every conflicting credential rejected, every non-Foundry
  `CLAUDE_CODE_USE_*` selector rejected; extend
  `explicit_providers_reject_active_incompatible_transports_only`.
- Codex command: exact `--config` arguments for resource and base-URL forms; conflicting
  credentials rejected; key never appears in argv.
- Endpoint validation: bad resource names and base URLs with credentials, query, or fragment.
- CLI help snapshot (`zeroshot/tests/native_v2_cli_help.rs`).

Live acceptance, against a real Foundry resource with one Claude and one OpenAI deployment:

1. Local Codex/foundry and Claude/foundry runs complete a small change.
2. One hosted interactive and one queued run on Platform with a stored `foundry` connection.
3. The key appears in no log, run record, argv, or observation output.

## Decisions

Made by the user on 2026-11-02, accepting the proposed defaults:

1. Provider name: `foundry`.
2. Connection field names: neutral `AZURE_FOUNDRY_*`, translated per harness.
3. The `LOCAL_ENVIRONMENT` passthrough fix ships with this change.
4. API keys only; Entra ID authentication is deferred.

`AZURE_FOUNDRY_BASE_URL` is the resource root without an API path. Claude appends `/anthropic`
and Codex appends `/openai/v1`, so one root serves both harnesses.

## Delivery shape

1. Zeroshot PR: protocol, Claude and Codex lanes, CLI, docs, tests.
2. Zeroshot Platform PR: connection row, health check, console docs.
