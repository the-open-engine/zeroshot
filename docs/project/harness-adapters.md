---
description: 'Available harnesses, local and target support, and the contract for contributed adapters.'
---

# Harness adapters

Official local builds include every adapter below. Install each native CLI separately for local use.

| Harness ID | Providers                                                           | Distribution |
| ---------- | ------------------------------------------------------------------- | ------------ |
| `codex`    | `openai`, `openrouter`, `gateway`, `bedrock`                        | Core         |
| `claude`   | `anthropic`, `openrouter`, `gateway`, `bedrock`                     | Core         |
| `copilot`  | `github`, `openai`, `anthropic`, `openrouter`, `gateway`, `bedrock` | Core         |
| `pi`       | `anthropic`, `openai`, `openrouter`, `gateway`, `bedrock`, `github` | Core         |
| `opencode` | `openai`, `anthropic`, `openrouter`, `bedrock`, `gateway`, `github` | Contributed  |
| `hermes`   | `openai`, `anthropic`, `openrouter`, `gateway`, `github`, `bedrock` | Contributed  |

Local builds enable `contributed-harnesses` by default. Platform and the default target image disable
default features and independently admit only the core registry. Targets advertise enabled pairs
through `openengine.runtime-harnesses/v1`; installing a CLI doesn't enable its adapter. Contributed
lanes require positive advertisement, while older targets without the extension retain legacy core
submissions.

Core adapters receive product maintenance. Contributors maintain their adapters; Zeroshot doesn't
guarantee contributed-harness compatibility with native releases. Both groups must implement the same
full execution, session and Active Memory contracts. Each runtime names its harness, provider and
model explicitly. Model identifiers remain provider-owned strings.

Gateway connections carry `GATEWAY_BASE_URL` and `GATEWAY_API_KEY`. Pi, Copilot, OpenCode and Hermes
also require `GATEWAY_API`: `openai-responses`, `openai-completions` or `anthropic-messages`.
Codex always uses Responses; Claude always uses Messages. Copilot's Bedrock lane uses AWS's
OpenAI-compatible Responses endpoint with `AWS_BEARER_TOKEN_BEDROCK` and `AWS_REGION`; an optional
`AWS_BEDROCK_BASE_URL` preserves a custom endpoint. Models must support the chosen native API.

Local native login works for Codex/OpenAI, Claude/Anthropic, Copilot/GitHub and its OpenAI,
Anthropic and OpenRouter BYOK lanes, and Pi/Anthropic, OpenAI and GitHub. OpenCode and Hermes retain
native authentication for every supported provider except gateway, including the native AWS
credential chain for Bedrock. Other lanes require declared connections. GitHub connections use
`COPILOT_GITHUB_TOKEN`; each harness owns its native exchange and refresh behavior. Declared
credentials use private runtime state and preserve saved logins.

Local OpenCode and Hermes runs preserve native configuration discovery with declared credentials.
Their declared authentication takes precedence over ambient authentication for the selected lane,
while native settings and plugins continue to apply. OpenCode keeps its original HOME/XDG paths and native
permission policy. Its declared authentication uses a private plugin, so `OPENCODE_PURE=true`
conflicts with that setup. Native plugins and custom transport code run with the native CLI's authority.
Local OpenCode adds no `--auto` flag and skips configuration inspection
because that inspection can start background dependency work.

Hermes uses the interpreter from its installed canonical executable and native configuration
factories without attaching to its shared gateway. Local profiles, plugins, MCP and permissions
remain native. Declared connections override native dotenv values. Remote backends retain native
containment limits. Missing native acknowledgement makes cleanup unconfirmed and fails execution;
unreported child usage makes totals incomplete.

## Contribute an adapter

Add an implementation under `zeroshot/src/harnesses/adapters/contributed/` and register it in that
directory's `register` function. Core implementations live beside it under `adapters/core/`;
existing public `native_v2_codex`, `native_v2_claude`, `native_v2_copilot` and `native_v2_pi` module
paths remain available to embedders.

A registration supplies a descriptor, provider-access declarations, local path normalization and a
factory. Keep descriptors pure: no credential reads, filesystem inspection, process launches or
provider requests. The registry validates bounded harness/provider IDs and compatible pairs.
Factories receive `HarnessBuildContext` and return `HarnessAdapter` with `NodeDriver` and
`SessionFactory` implementations.

Adapters own native protocols, authentication translation and permission handling, including their
environment allowlists and home/path expansion. Use shared process and session helpers for private
execution files, bounded streaming, cancellation and cleanup. Support workers and verifiers with
authoritative response validation, bounded corrections, reusable sessions, live output and usage
that reports uncertainty when totals are incomplete.

Install the native Active Memory hook, plugin or bridge in the adapter. Shared memory owns recall
budgets, receipts and maintenance; missing acknowledgement means unknown delivery evidence.

Run focused fixtures and shared registry/session tests. The ignored contract in
`harnesses/live_tests.rs` requires a disposable test repository and explicit `ZEROSHOT_LIVE_HARNESS`,
`ZEROSHOT_LIVE_PROVIDER`, `ZEROSHOT_LIVE_MODEL` and `ZEROSHOT_LIVE_WORKSPACE` values. Set
`ZEROSHOT_LIVE_CLI_BINARY` to a separately built CLI's absolute path for memory callbacks. Optional
`ZEROSHOT_LIVE_CONNECTION_FIELDS` lists additional declared field names, separated by commas.
Test native bridges too, and check the core-only library:

```bash
cargo check --locked --package zeroshot --lib --no-default-features
```
