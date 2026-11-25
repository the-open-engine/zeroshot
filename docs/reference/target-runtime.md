---
description: 'Target discovery, local harness configuration, permissions and workspace export.'
---

# Target runtime contracts

[Targets](../concepts/targets.md) covers choosing and connecting a target.

## Wire compatibility

OECP uses `openengine.cluster/v3`. Direct and hosted discovery use
`zeroshot.native-v2-target/v4`. Update the client and target together when upgrading
from a release that addresses run parts separately.

Run status returns the original `runId`, the selected `partId` and `currentPartId`.
Resume includes the observed `partId` and the proposed `successorPartId`. A stale source
returns `RUN_CONFLICT` with the current part ID. Log and watch cursors include the
originating part and can be passed back unchanged.

Hosted run routes use `zeroshot.hosted-runs/v3` for status, watch, logs and force in every phase.
For attach and steer, the CLI opens an OECP session scoped to the selected physical part.
A guarded steer must still address the current part when the host accepts it.

Active execution `startedAt` and watch notification `recordedAt` are Unix
milliseconds when known. Python users upgrade the native client by upgrading the
`the-open-engine-zeroshot` wheel.

## Discovery and sizes

Targets advertise their enabled harness/provider pairs through the
`openengine.runtime-harnesses/v1` discovery extension. The CLI checks every selected lane before
requesting an access token or submitting a run or profile, and the target enforces its catalog
again during admission. Older targets without this extension accept only the established core
harnesses through client preflight.

Sizes are target-owned identifiers of 1–128 non-control Unicode characters. A target advertises
its catalog through `extensions.run_size_catalog`:

```json
{
  "kind": "openengine.run-sizes/v2",
  "default": "standard",
  "sizes": [
    { "id": "standard", "description": "4 vCPU, 16 GiB RAM, 128 GiB disk" },
    { "id": "big-box", "description": "32 vCPU, 128 GiB RAM, 1024 GiB disk" }
  ]
}
```

List a named target's identifiers and descriptions with `zeroshot target sizes NAME`.
The default output is a table that wraps to terminal width; add `--json` for the structured catalog.
Hosted targets may return the current organization's enabled catalog through authenticated discovery.

The CLI and SDK forward an explicitly requested identifier unchanged. They do not maintain size
names or reject a request because a discovery list is missing, empty, or stale. The target decides
availability and rejects unsupported or disabled sizes. An omitted or null size selects the target's
default at admission; the accepted run stores that concrete size. Requests retain their original
identity, so an exact retry still returns the accepted run after a catalog/default change.

An empty catalog offers no sizes and has no default. Local execution and `target serve` offer `host`,
which uses the current host's CPU, memory and storage. Private capsules preserve the concrete size
already admitted by their host. The descriptor extension uses its own key so older string-based size
advertisements cannot break unrelated discovery operations. Size is shared by the graph's agents,
independently of their harness/provider lanes; zeroshot defines no AWS shapes or billing prices.

Official local builds include contributed adapters; Platform and the default target image enable
only core adapters. See [Harness adapters](../project/harness-adapters.md) for availability and
provider support.

## Saved-profile imports

A host that expands saved-profile dependencies during its existing profile-run operation may
advertise this independent optional discovery extension:

```json
{
  "saved_profile_imports": {
    "kind": "openengine.saved-profile-imports/v1"
  }
}
```

The marker contains only `kind`. Its absence or a different kind keeps imported saved profiles
unsupported by the CLI. It adds no routes and changes no compatibility versions, profile
selectors, authentication or submission identities. The CLI fetches the selected parent's preview,
checks this capability, then sends the existing `RunProfileRunRequest` with its `profile` selector.
It never fetches child profiles or sends expanded bodies. Profiles without imports keep their
existing behavior. Authored inline graphs and local profiles with imports still require local
execution. `--validate-only` refuses remote imports because complete validation requires host
admission; it does not report an unresolved dependency graph as valid.

Hosts use `zeroshot_engine::native_v2_admission::profile_expansion::{expand_profile_imports,
ProfileExpansionError}`. The typed function takes the authored parent `GraphSpec`, `RuntimePlan`,
its optional root `RunProfileName` (supply it for a saved parent), and a synchronous read-only
catalog lookup. For example, with a host-owned `BTreeMap<RunProfileName, (GraphSpec, RuntimePlan)>`:

```rust
let (graph, runtime) = expand_profile_imports(
    parent_graph,
    parent_runtime,
    Some(&parent_name),
    |name| Ok(catalog.get(name).cloned()),
)?;
```

Resolve and authorize the parent selector and every dependency from **one immutable snapshot** in
the parent's scope. Unqualified same-scope names are sufficient; storage and authorization remain
host responsibilities. Do not assemble a catalog from successive mutable remote reads or accept
client-supplied `body`/`initialInput` expansions. The helper rejects supplied expansions, missing
profiles, cycles, incompatible child policies and expanded node/depth/import-byte limits. It
preserves input bindings and child schemas, qualifies node/control/worker/network identities
deterministically, and materializes each agent's effective lane relative to the parent runtime.
The local profile store uses this same linker while preserving its raw stored-byte limit.

After expansion, apply the host's usual provider-access materialization, runtime catalog, connection
selection, graph/input verification, delivery policy and admitted-definition byte limit before
allocating a run. Persist the expanded graph and runtime as the admitted/recovery definition; retain
authored references in profile storage and editors. The helper does not perform admission or grant
authority. Advertise the marker only when the profile-run handler follows this contract, including
when callers bypass the CLI. Existing MCP tool definitions and shared skills need no changes;
hosted services adopt the helper and capability with their native dependency update.

## Hosted workspace checkpoint opt-out

`RuntimePlan.workspaceCheckpoints` defaults to `true`. A compatible named target may explicitly
allow `false` per run with this independent discovery extension:

```json
{
  "workspace_checkpoint_opt_out": {
    "kind": "openengine.workspace-checkpoint-opt-out/v1"
  }
}
```

The strict marker contains only `kind`. It does not change `openengine.workspace-checkpoints/v1`,
recovery routes, wire versions or authentication. Missing or different versions retain refusal.
New CLI clients check it before resolving access tokens or sending run/profile bodies; an older
client still refuses opt-out. The Python SDK forwards `UniformRuntime.workspace_checkpoints=False` or an exact runtime
containing `workspaceCheckpoints: false` through its bundled CLI and therefore needs the matching upgraded CLI too.

Host integration uses these Rust APIs:

- Set `ProductionHostingConfig.workspace_checkpoint_opt_out = true` **and** install
  `workspace_storage: Some(Arc<dyn HostedWorkspaceStorage>)`. Both are required; defaults and
  `zeroshot target serve` continue to require checkpoints. The production factory installs trusted
  admission policy and advertises the marker. Gate any public hosted gateway's advertisement and
  admission on the same host policy, using `TargetDiscoveryDocument::with_workspace_checkpoint_opt_out()`.
- For host-owned profile validation, set
  `NativeV2AdmissionContext.hosted_workspace_checkpoint_opt_out = Some(enabled)` and use
  `NativeV2Admission::validate_profile_with_context`. Production run admission applies that policy
  independently, even when a client bypasses discovery. `None` retains local admission semantics.
- `HostedWorkspaceStorage::prepare` receives the admitted boolean as
  `HostedWorkspaceRequest.workspace_checkpoints`. When false, skip workspace restore/snapshot
  preparation and return a host-owned `RunCheckpointStore` that retains metadata without copying
  workspace bytes. The existing lifecycle still calls `enter` at engine boundaries, `finish` on
  failure, and `discard` after success. Preserve durable execution/final histories, output, comment
  publications and delivery receipts independently, including after `discard`. Conversation capture
  is skipped by the supervisor. Do not couple final history publication to a workspace backup.
- For the template catalog (Cargo feature `workspace`), use
  `workspace::catalog_with_checkpoint_policy(&harnesses, enabled)`. True preserves code-review's
  supplied `workspaceCheckpoints: false`; existing `catalog()` and `catalog_with_harnesses()` keep
  checkpoint defaults enabled for legacy consumers. `profile_setup[_with_harnesses]` preserves
  the selected setting while materializing contained provider access.

Opted-out hosted graphs reject every request or wait node, including nested groups and expanded
profile imports, even on an unreachable authored path. These nodes can require persisted
continuations or suspension. Manual suspension and recovery are also refused for such runs.
In-run conversation reuse still works, but conversations cannot be restored across runs without
snapshots. Ordinary history, outputs, usage and logs remain available.
Local checkpoint opt-out, restart on the retained checkout and explicit discard continuations
retain their existing behavior.

Use matching release versions of the runtime and protocol crates, target and gateway, and CLI and
Python clients. Advertise opt-out only when admission, storage and catalog policy agree.

## Local workspace and authentication

Omit `--target` to work in an existing local folder. `--cwd FOLDER` selects the folder for one run;
the default is the invoking folder. Input, graph, runtime, and environment file arguments remain
relative to the invoking folder. A folder inside a Git checkout resolves to its canonical checkout
root. Resume uses the original stored workspace.

A run without Git delivery also accepts plain folders, remoteless repositories, detached checkouts,
and unborn repositories. zeroshot starts a detached controller and keeps the ledger under the local
state directory, where later CLI invocations can list, watch, or stop the run. It does not initialize
Git to run in a plain folder.

The workspace and active zeroshot state directory must be disjoint, including symlink aliases.
If either contains the other, select another folder or set `ZEROSHOT_STATE_DIR` outside the workspace.
Existing inactive state folders do not impose a separate admission rule. Disjoint workspaces can
run concurrently. Aliases share one lease; a folder and its subtree cannot run at the same time. Reusing a submission key for
another folder returns a conflict rather than a run from the first folder.

Local execution is the only mode that mutates the caller's existing folder; each selected
installed harness runs as the current user. A plan with
[per-node lanes](runtime-plan.md#per-node-lanes) needs every lane's CLI installed. The
adapter declares whether its lane can reuse the CLI's native login; otherwise it needs the provider's
[connection values](runtime-plan.md#connections). Declared credentials take precedence
over native login state. Before the controller starts,
`zeroshot run` and `zeroshot resume` check that each lane's executable is on `PATH`; they do not
check login state, so a missing login fails at that lane's first turn.

### Local Codex

Local Codex runs with `provider: "openai"` use the model provider and transport configured in the
user's Codex configuration, including OpenAI-compatible proxies such as LiteLLM. zeroshot supplies
the runtime's model and any explicit reasoning effort, but leaves web search and sandbox network
access to Codex's configuration. Explicit `openrouter`, `bedrock`, `foundry`, and `gateway` selections configure
those providers; hosted targets use their own isolated provider setup.

### Local Claude

Local Claude runs use the current user's home and `CLAUDE_CONFIG_DIR`, when set. Local runs
also inherit the shell's endpoint variables, including `OPENAI_BASE_URL`
for Codex and `ANTHROPIC_BASE_URL` for Claude. Declared connection values take precedence over these
shell defaults. Hosted adapters receive declared connection values without inheriting the host's
shell settings. Explicit gateway selections use their `GATEWAY_BASE_URL` connection value. Local
`anthropic` runs also inherit Claude's shell provider-selection flags. Explicit `openrouter`,
`bedrock`, `foundry`, and `gateway` selections retain their provider setup and reject declared transport flags
that would route to an incompatible provider.

For Copilot login, provider configuration and token refresh, see [GitHub Copilot](#github-copilot).

### Local Pi

Local Pi runs use the current user's `~/.pi/agent` for a `pi/anthropic`, `pi/openai` or `pi/github` lane, which
reuses a stored Pi login. Any other Pi lane, a declared credential, or a caller-owned endpoint
instead receives a private agent directory so no ambient catalog or credential is inherited. A
declared or ambient `ANTHROPIC_BASE_URL`, `OPENAI_BASE_URL`, or `OPENAI_API_BASE` becomes a Pi
provider override that keeps that provider's own protocol, and `pi/gateway` carries
`GATEWAY_BASE_URL`, `GATEWAY_API_KEY`, and an authored `GATEWAY_API` wire protocol. Pi exposes no
permission control, so Pi runs add no bypass flag; containment remains the boundary.

### Permissions and process context

The local target intentionally has the same-user trust boundary as running the harness CLI directly:
native config, user files, and credential-store access remain subject to that harness's own sandbox
and tool policy. zeroshot transfers the invoking shell through a private, one-shot bootstrap and
deletes it before the controller performs run effects; the detached controller itself receives only
minimal platform environment. A later local CLI startup removes a bootstrap orphaned beyond the
controller handoff window. This snapshot is never written to a run ledger or sent to a hosted target.

Hosted Codex and Claude workers and verifiers always run with
`--dangerously-bypass-approvals-and-sandbox` or `--dangerously-skip-permissions`, respectively.
zeroshot does not inspect the harness's native permission policy for hosted turns; the disposable
capsule is the isolation boundary.

Local Codex and Claude workers and verifiers use those flags only when no permission policy is
configured. Before each local turn, zeroshot queries the harness's resolved settings without sending
a model prompt. Explicit approval, sandbox, permission rules, and managed restrictions take
precedence, so zeroshot adds no bypass flag in those cases. Claude shell permission controls are
inherited locally too. If inspection fails or the CLI does not support it, the harness keeps its
native permission behavior. Inspection adds one CLI startup per local turn and is bounded to sixty
seconds. Whenever inspection is unavailable, for example because the CLI cannot start, exits early,
returns an error or an incomplete or oversized answer, or exceeds the sixty seconds, `zeroshot logs`
shows an error-level line for that node naming the harness and a bounded, redacted cause, and
stating that zeroshot is continuing without its automatic permission bypass. That line does not
change the node's outcome: the turn still runs with native permissions and its own result decides
success. Native overrides and configured policy are not inspection failures and are not reported.
The probe must finish cleanup before the model turn starts; unconfirmed cleanup stops the run and
identifies configuration inspection in its diagnostic. Inspection does not rewrite settings files,
though the CLI may update its own startup state.

When a Claude Code result reports tool calls that its permission settings or hooks denied, in local
or hosted runs, `zeroshot logs` shows an error-level line for that node with the denied tool names
and counts, never the calls' inputs. Denied calls do not change the node's outcome: its result and
the graph's verifiers decide.

### Reviewer access

Every agent verifier node, including those in custom profiles, uses the same permission handling as
workers. zeroshot automatically adds instructions to inspect without modifying source, tests,
configuration, or other material under review or implementing repairs. Verifiers may run checks and
create temporary files and generated artifacts, and must report failed or unavailable checks accurately.
The built-in acceptance verifier also tries to run the repository's pre-commit checks and reports
the results.

Local and hosted verifiers operate directly on the candidate: the restriction on edits is instruction
guidance, not a filesystem boundary. Hosted nodes share the prepared workspace and its owner identity.
They retain separate session homes and process cleanup markers, so ending one node does not terminate
another node or the run's startup services.

### Host context

Hosting applications can supply nonsecret machine facts and operating guidance through
`ProductionHostingConfig.agent_context`, using an `AgentContextProvider`. zeroshot samples this
context before every agent execution, including resumed attempts and auxiliary agents, and adds
it to the prompt for every harness. The context is transient: it does not change the saved graph,
runtime plan, or durable invocation. Local and direct targets attach no host context by default.

### Codex search and provider configuration

zeroshot controls the response format and session continuation. Web search follows Codex's
configuration: by default it uses live search with full access and cached search otherwise. For local
runs, an explicit `web_search="cached"` prevents zeroshot from adding bypass because Codex can
otherwise promote cached search to live search under full access. Hosted turns always use maximum
bypass inside the disposable capsule, so Codex may promote cached search to live. Command network
access follows the selected sandbox policy locally; hosted command isolation comes from the capsule.

For local Codex, zeroshot reads the active provider's `env_key` and environment-based header names
from Codex's resolved configuration, then forwards only those values from the invoking shell to the
model process. This lets custom OpenAI-compatible gateways keep using their existing Codex setup.
Declared connection values take precedence, and a declared `OPENAI_API_KEY` remains available under
that name. zeroshot also supplies `CODEX_API_KEY` for native OpenAI authentication when needed.
Values discovered from local configuration are not persisted or sent to hosted targets. See
[Runtimes and connections](../concepts/runtimes-and-connections.md).

## GitHub Copilot

Copilot's `github` lane uses your GitHub user identity and Copilot entitlement. Local runs on that
lane reuse the login stored by `copilot login` in `COPILOT_HOME` or the system credential store. Ambient
`COPILOT_GITHUB_TOKEN`, `GH_TOKEN`, or `GITHUB_TOKEN` values stay in private RPC and are excluded
from agent tool environments. For a contained target, store a user OAuth token or fine-grained
personal token with Copilot Requests permission in the `github` connection's
`COPILOT_GITHUB_TOKEN` field. A GitHub App installation token is not the user-backed route.

Local Copilot runs use the current user's `COPILOT_HOME` and system credential store. Ambient
`COPILOT_GITHUB_TOKEN`, `GH_TOKEN`, or `GITHUB_TOKEN` values are selected in native precedence and
sent through private RPC rather than the Copilot process or its tools. Copilot endpoint, proxy,
certificate, and custom-provider environment settings are inherited locally. Static BYOK endpoint,
credential, header, wire, and model-mapping settings are materialized into the private Copilot RPC
session and removed from agent tool environments. `COPILOT_PROVIDER_API_KEY_COMMAND` crosses the
singular private provider contract, which lets the pinned CLI refresh rotating keys for each request.
Eligible helper-only invoking-shell fields are all marked through Copilot's `--secret-env-vars`, so
they remain available to the internal provider command but are not inherited by agent shell or MCP
environments. Runtime, auth, provider, and parent-process loader controls are excluded.
That filtering is not a separate OS identity boundary against adversarial same-UID processes.
A bounded `COPILOT_PROVIDERS_CONFIG` or default `COPILOT_HOME/providers.json` registry crosses as
protocol provider/model values because the headless server does not import that path into sessions;
a nonempty registry wins over legacy provider variables. An explicit declared token suppresses
ambient BYOK and offline controls. Declared endpoint and credential fields cannot accidentally retain
ambient credentials for a different route. The runtime model remains the process-level registry
selection while provider `modelId` capability and wire-model mappings stay intact. A registry
command translated to the singular RPC contract uses its authored `modelId` as that session's model.
Contained targets use a private Copilot home and require the canonical `COPILOT_GITHUB_TOKEN`
connection instead.

```json
{ "harness": "copilot", "provider": "github", "model": "auto" }
```

Model IDs pass unchanged to the Copilot process. When a registry entry with `apiKeyCommand` must use
Copilot's singular provider contract, its authored `modelId` becomes the RPC session model and its
authored `wireModel` still names the provider request model. zeroshot uses headless RPC schema output
and validates each response locally, allowing at most two correction turns in the same session.
The target image already includes the native Copilot CLI.

Hosted connection resolvers may additionally declare `COPILOT_GITHUB_TOKEN_EXPIRES_AT` (Unix
seconds). Copilot then requests credentials through the resolver throughout a long execution.
The callback must match `COPILOT_GH_HOST` (or `GH_HOST`) when configured. The resolver must return a
refreshed token with more than one hour remaining. Static tokens
without expiry metadata are supplied once when each provider process starts.

## Workspace export protocol

Hosted targets can advertise workspace downloads independently of resume/checkpoints. Add this
outer discovery extension alongside the existing `hosted_runs` extension:

```json
{
  "workspace_export": {
    "kind": "openengine.workspace-export/v1",
    "route_template": "/native-v2/runs/{run_id}/export-workspace"
  }
}
```

`TargetWorkspaceExportDiscovery` is the authoritative Rust type. The extension object is strict;
unknown outer extension keys remain compatible with older clients. Do not add export fields to
`zeroshot.hosted-runs/v3` or `openengine.hosted-workspace-recovery/v1`. Export requires the exact
kind above; an absent or unfamiliar kind disables it without a fallback operation.

The route uses the same-origin `hosted_runs.base_url`, including any installation path prefix,
just like hosted recovery routes. It must contain exactly one `{run_id}` path segment, no other
variables, query, fragment, traversal, or origin. The client escapes the run identity as one segment
and never follows redirects.

The client sends an authenticated POST with `Accept: application/x-tar`, `Cache-Control: no-store`,
and a strict JSON `RunExportWorkspaceParams` body:

```json
{ "runId": "RUN_ID", "fromCheckpoint": "CHECKPOINT_ID" }
```

Omit `fromCheckpoint` for the latest retained workspace. Path and body run identity must agree.
Checkpoint IDs select entry snapshots and remain opaque. The server owns authorization, retention,
checkpoint selection and storage/decryption credentials. The request accepts no storage URLs,
paths, repository prefixes or export filters. Existing hosted OAuth session organization selection
and API-key `X-Zeroshot-Organization` handling apply unchanged.

Success is `200 OK`, `Content-Type: application/x-tar`, `Cache-Control: no-store`, with raw streamed
tar bytes. HTTP/1 transfers need a Content-Length without Transfer-Encoding, or a single chunked
transfer coding (case-insensitive). Transfer-Encoding takes precedence over Content-Length;
the CLI rejects unsupported codings. Close-delimited responses cannot prove exporter success and
are rejected. See [RFC 9112 framing](https://www.rfc-editor.org/rfc/rfc9112.html#section-6.3).
HTTP/2 uses its native stream completion. The server must finish HTTP framing only after its
exporter exits successfully and retained-state availability
is rechecked. An exporter/storage failure after bytes were emitted must abort the transfer, even
when those bytes already form a complete tar. Failures before streaming use the existing bounded
`TargetHttpProblem` response and a non-success status.

The client consumes the entire transfer before publishing a private file without replacement;
HTTP/body errors, truncated framing and cancellation remove temporary output. It does not trust a
server filename, extract files, resume a run or receive storage keys. This is an HTTP streaming
contract, not a new OECP method. The generated protocol schema includes its request body.
