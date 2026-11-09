---
description: 'Available harnesses, local and target support, and the contract for contributed adapters.'
---

# Harness adapters

Official local builds include every adapter below. Install each native CLI separately for local use.

| Harness ID | Providers                                                                                      | Distribution |
| ---------- | ---------------------------------------------------------------------------------------------- | ------------ |
| `codex`    | `openai`, `openrouter`, `gateway`, `bedrock`, `foundry`                                        | Core         |
| `claude`   | `anthropic`, `openrouter`, `gateway`, `bedrock`, `foundry`                                     | Core         |
| `copilot`  | `github`, `openai`, `anthropic`, `openrouter`, `gateway`, `bedrock`                            | Core         |
| `pi`       | `anthropic`, `openai`, `openrouter`, `gateway`, `bedrock`, `github`                            | Core         |
| `opencode` | `openai`, `anthropic`, `openrouter`, `bedrock`, `gateway`, `github`, `opencode`, `opencode-go` | Contributed  |
| `hermes`   | `openai`, `anthropic`, `openrouter`, `gateway`, `github`, `bedrock`                            | Contributed  |

The `claude` adapter requires Claude Code 2.1.310 or newer. Before its first model turn it runs the
selected executable with `--version` and fails the node, naming the reported version, when that
release is older or the output can't be read.

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
Codex and Claude `foundry` lanes share one Microsoft Foundry connection; see
[Microsoft Foundry](../concepts/runtimes-and-connections.md#microsoft-foundry).

Local native login works for Codex/OpenAI, Claude/Anthropic, Copilot/GitHub and its OpenAI,
Anthropic and OpenRouter BYOK lanes, and Pi/Anthropic, OpenAI and GitHub. OpenCode and Hermes retain
native authentication for every supported provider except gateway, including the native AWS
credential chain for Bedrock. OpenCode's `opencode` (Zen) and `opencode-go` (Go) lanes reuse the
native OpenCode account login; a declared `OPENCODE_API_KEY` serves both. Other lanes require
declared connections. GitHub connections use
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

## Execution environment

Every per-node harness child process receives these public, nonsecret execution labels, including
agent verifiers and host-owned auxiliary agents. Local and contained launches use the same contract.

| Variable                    | Value                                                                                                                            |
| --------------------------- | -------------------------------------------------------------------------------------------------------------------------------- |
| `ZEROSHOT_RUN_ID`           | Owning run's existing ID.                                                                                                        |
| `ZEROSHOT_NODE_NAME`        | Authored node name, or the auxiliary agent's host-owned name.                                                                    |
| `ZEROSHOT_NODE_KIND`        | Graph kind: `step` or `verifier`.                                                                                                |
| `ZEROSHOT_NODE_INSTANCE_ID` | Existing run-scoped node instance; mapped items have distinct identities, while retries and loop revisits retain their instance. |
| `ZEROSHOT_EXECUTION_REF`    | Opaque public execution selector shared with logs, attach and diagnostics.                                                       |
| `ZEROSHOT_NODE_ATTEMPT`     | One-based reducer attempt within the current node visit.                                                                         |
| `ZEROSHOT_NODE_ITERATION`   | One-based iteration of the innermost enclosing authored loop; `0` outside loops.                                                 |

For outer iteration 3 and inner iteration 2, a node inside the inner loop sees iteration `2`.
A node in the outer body after the inner loop sees `3`. The inner count restarts at `1` on the
next outer visit; a node after all loops sees `0`. Map indices do not affect iteration values.

Reducer retries keep the iteration and advance the attempt. Fresh loop visits follow the reducer's
attempt reset. Harness continuations, provider retries, structured-output corrections and configuration
inspection launches retain the dispatch metadata; spawning a process never advances either number.
Auxiliary agents use their own run, node and execution identities, iteration `0` outside authored
loops, and attempt `1` unless the host request explicitly represents a retry of that visit.

Zeroshot applies these labels after environment merges, replacing stale incoming values, with
case-insensitive names on Windows. It never changes the controller's process environment or puts
labels in credential connections. Descendant tools and hooks may inherit them subject to native
harness filtering; remote tool backends retain their own environment policies. Descendants can keep
or change inherited labels, so they are not secrets or evidence of authorization. Native control,
decision and delivery nodes do not acquire a fabricated harness process.

## Contribute an adapter

Add an implementation under `zeroshot/src/harnesses/adapters/contributed/` and register it in that
directory's `register` function. Core implementations live beside it under `adapters/core/`;
existing public `native_v2_codex`, `native_v2_claude`, `native_v2_copilot` and `native_v2_pi` module
paths remain available to embedders.

A registration supplies a descriptor, provider-access declarations, local path normalization, pure
reserved-control validation for `harnessConfig`, and a factory. Keep descriptors pure: no credential reads, filesystem inspection, process launches or
provider requests. The registry validates bounded harness/provider IDs and compatible pairs.
Factories receive `HarnessBuildContext` and return `HarnessAdapter` with `NodeDriver` and
`SessionFactory` implementations.

Adapters own native protocols, authentication translation and permission handling, including their
environment allowlists and home/path expansion. Use shared process and session helpers for private
execution files, bounded streaming, cancellation and cleanup. Apply the shared
`provider_process::apply_execution_environment` helper after environment merges for every agent launch. Support workers and verifiers with
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

## Native configuration ownership

All registered adapters implement [per-node native configuration](../reference/runtime-plan.md#per-node-native-harness-configuration).
Keep carrier parsing shared and reserved controls adapter-local. Bind configuration at the invocation
or session boundary, never on the lane-wide adapter. Native APIs use object options; CLI arguments
remain argument tokens. Preserve the same configuration on corrections, retries and session resumes.
Leave unknown native names and values to native initialization. Do not add an option allowlist, portable
permission policy, native validation prompt, or generalized inspection subprocess.

Owned authentication, transport, response schemas, identity, memory hooks and containment cannot be
replaced through overrides. Existing permission inspection only decides whether to add an automatic
local bypass. Native initialization failure must preserve its bounded redacted cause and node/harness
identity, settle cleanup, and skip automatic correction/fallback/retry. Focused process/session tests
must cover forwarding, isolation, reuse and rejection for every registered adapter.
