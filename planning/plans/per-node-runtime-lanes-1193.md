# Per-node harness and provider lanes, issue 1193

Status: approved on 2026-10-05. Pull request 1 in [Delivery shape](#delivery-shape) implements the
core and pull request 2 the UI. Sections corrected during implementation describe the code as
built.

Authority: [zeroshot issue 1193](https://github.com/the-open-engine/zeroshot/issues/1193),
"Choose the harness and provider per node in a RuntimePlan". Decisions recorded in
[Decisions](#decisions) were made by the user on 2026-10-05.

## Problem

A `RuntimePlan` selects one harness and one provider for the whole run. Model, effort, session
scope, and connections are already per node. A profile can therefore run every node on Claude Code
with different models, but it cannot have Codex implement and Claude Code review. Reviewers from
the implementer's model family share its blind spots, and users with their own agent pipelines ask
for cross-vendor review directly.

The restriction is not an auth, capsule, or session constraint. It has two sources:

- `RuntimePlan` is a serde enum tagged by `harness`, and each variant carries its own provider
  enum (`crates/openengine-cluster-protocol/src/native_v2_run/wire.rs`).
- The runtime builds exactly one agent adapter per run (`NativeV2HarnessConfig` in
  `zeroshot/src/native_v2_candidate.rs`), and the two composition roots pick it from the plan
  variant (`local_harness` in `zeroshot/src/native_v2_local.rs`, `harness` in
  `zeroshot/src/native_v2_hosting/allocator.rs`).

Everything downstream is already keyed per node: session pool keys, provider private homes, hosted
UID markers, admission rules, status and observation, Cloud submission, the Python SDK, and the
target image, which installs all three CLIs at fixed paths.

## Decisions

1. The override is a nested object on the agent binding, not flat `harness` and `provider`
   strings. Serde cannot flatten a tagged enum inside an enum variant, so flat fields would need a
   hand-written deserializer and would lose the typed harness/provider pair check.
2. The runtime builds one adapter per distinct harness and provider pair. Adapters keep their
   provider fixed at construction; provider-per-invocation is out of scope.
3. Backwards compatibility is a target. Plans without overrides stay byte-identical. A new CLI
   refuses to send a plan with overrides to a target that does not advertise support.
4. Local preflight checks every lane up front.
5. Hosted connection requirements are the union across lanes, through the existing mechanism.
6. In the UI, changing the run-level harness clears every per-node override. Changing the
   run-level provider leaves them.
7. The run-level pair stays. Dropping it would be a breaking wire change for profiles, ledgers,
   history, fixtures, docs, and Cloud. This design is a stepping stone: a later major version may
   make the run-level pair optional when every agent node carries a lane, without undoing any of
   this work.

## Acceptance

1. A plan whose `worker` binds `codex`/`openai` and whose `acceptance` binds `claude`/`anthropic`
   admits, runs locally with both CLIs installed, and runs on a hosted target. Each node spawns
   the CLI of its own lane.
2. A plan without any `lane` serializes byte for byte as today, admits on a target built before
   this change, and produces the same hosted submission digest.
3. A plan with a `lane` sent to a target that does not advertise
   `openengine.node-runtime-lanes/v1` fails before any request body is sent, with an error that
   names the capability.
4. A local run whose plan needs an executable that is not on the captured shell path fails before
   the controller launches, with an error that names the lane and the executable.
5. In the UI, an agent node can override its harness and provider, return to the run default,
   keeps its override when the run-level provider changes, loses it when the run-level harness
   changes, and the history view shows each node's effective lane when any node overrides.
6. ACP accepts a profile that mixes Codex and Claude lanes and rejects one with a Copilot lane.
7. Every validation lane in `CLAUDE.md` passes, including the protocol schema check and the strict
   docs build.

## Wire contract

Protocol crate: `crates/openengine-cluster-protocol`.

### `RuntimeLane`

A new public enum in `native_v2_run`, internally tagged by `harness` with the provider enum typed
per variant, exactly like the three `RuntimePlan` variants:

```rust
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields, tag = "harness", rename_all = "snake_case")]
pub enum RuntimeLane {
    Copilot { provider: CopilotProvider },
    Codex { provider: CodexProvider },
    Claude { provider: ClaudeProvider },
}
```

`CopilotProvider`, `CodexProvider`, and `ClaudeProvider` gain `Hash`, `Ord`, and `PartialOrd` so a
lane can key a `BTreeMap` or `BTreeSet`. Invalid pairs such as `codex` with `anthropic` are
rejected by deserialization, as they are for the run-level pair today.

### Agent binding

`NodeRuntimeBinding::Agent` gains one optional field:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
lane: Option<RuntimeLane>,
```

Wire form:

```json
{
  "kind": "agent",
  "lane": { "harness": "claude", "provider": "anthropic" },
  "model": "REVIEW_MODEL_ID",
  "effort": "high"
}
```

`git_delivery` bindings do not gain the field. A `lane` on one is rejected as an unknown field.
An override equal to the run-level lane is accepted and means the same as no override.

### `RuntimePlan` accessors

`RuntimePlan` keeps its shape, its `deny_unknown_fields`, and its existing `size`, `nodes`, and
`connection_requirements` methods. It gains:

| Method                                                                | Returns                                                                  |
| --------------------------------------------------------------------- | ------------------------------------------------------------------------ |
| `lane(&self) -> RuntimeLane`                                          | The run-level lane                                                       |
| `effective_lane(&self, &NodeRuntimeBinding) -> Option<RuntimeLane>`   | The binding's `lane`, else the run-level lane; `None` for `git_delivery` |
| `lanes(&self) -> BTreeSet<RuntimeLane>`                               | Distinct effective lanes across all agent bindings                       |
| `has_lane_overrides(&self) -> bool`                                   | Whether any agent binding carries its own `lane`, even the run-level one |
| `nodes_mut(&mut self) -> &mut BTreeMap<NodeName, NodeRuntimeBinding>` | Mutable node access                                                      |

`has_lane_overrides` sits beside `connection_requirements` in the protocol crate, so a host that
depends only on the protocol crate can apply the same [client check](#client-check) as the CLI.

Every product consumer that destructures the three plan variants today moves to these accessors.
`nodes_mut` replaces the two private helpers that do the same destructuring in
`zeroshot/src/native_v2_candidate/provider_access.rs` and
`zeroshot/src/native_v2_cli/execution/submission.rs`. The only remaining variant matches are the
ones that construct a plan.

`zeroshot/src/native_v2_contract.rs` re-exports `RuntimeLane` and updates its module doc, which
currently says the contract binds leaves to one graph-wide lane.

### Generated schema

`protocol/openengine-cluster/v1/schema.json` is regenerated with
`cargo run -p openengine-cluster-testkit --bin generate-cluster-protocol -- --write`, and
`npm run protocol:check` must pass. The change is additive: one optional property on the agent
binding and one new definition.

## Target compatibility

### Discovery marker

The target discovery document (`crates/openengine-cluster-protocol/src/native_v2_target.rs`)
gains a marker extension that follows the existing `workspace_recovery` pattern:

```rust
pub const NODE_RUNTIME_LANES_KIND: &str = "openengine.node-runtime-lanes/v1";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TargetNodeRuntimeLanesDiscovery {
    pub kind: String,
}
```

`TargetDiscoveryExtensions` gains `node_runtime_lanes: Option<TargetNodeRuntimeLanesDiscovery>`
with `skip_serializing_if = "Option::is_none"`. Targets built from this change always advertise
it. Cloud is a hosted target behind the same discovery document, so it is covered once it serves
the marker and is refused cleanly until then.

The marker's shape is frozen at exactly `{"kind": ...}`. Like its sibling markers it denies unknown
fields, so a marker object with an extra field fails the whole discovery document for every client
that reads it. A later capability that needs data, for example which harnesses a target can run,
gets its own new extension key instead: `TargetDiscoveryExtensions` ignores unknown keys, so older
clients keep parsing discovery, and that map must keep ignoring them. This follows the precedent of
hosted workspace recovery, which got its own extension rather than changing the strict hosted-runs
object. An absent marker, or one of a different kind, never fails discovery: a plan with lanes is
refused and a plan without lanes works.

### Client check

`RuntimePlan::has_lane_overrides` reports whether a plan carries any per-node lane. It lives in the
protocol crate beside `connection_requirements`, so hosts that depend only on the protocol crate
can apply the same check. The controller authority
(`zeroshot/src/native_v2_target/controller_authority/`) applies it before:

- any hosted or direct-target run submission;
- any remote profile set. Direct targets reject every profile operation, so this is a hosted
  check. Running a stored profile needs none because its plan already lives on the target.

If the plan carries a lane and the target does not advertise the marker, or advertises it with a
different `kind`, the operation fails with a `TargetAuthorityError` that names the capability:
"target does not support per-node runtime lanes (openengine.node-runtime-lanes/v1)". A different
`kind` counts as unsupported. Unlike the workspace capabilities it is not a discovery error, so
plans without lanes keep working with every target.

The check runs once the target's descriptor is resolved and before any access token is acquired.
A hosted token request may POST a refresh exchange, so a refusal sends no request body at all:
no token exchange and no run or profile POST. Local runs and `--validate-only` need no check
because they never leave the binary.

### What older readers do

An older binary that reads a lane-bearing plan from `profiles.json`, a ledger, or a history export
rejects it on the unknown field. That is fail-closed and cannot be improved from this side. An
older target receiving such a plan is prevented by the client check above.

`profiles.json` holds every local profile and is parsed as a whole, so one lane-bearing profile
makes the entire local profile store unreadable to an older binary, which reports only that the
store is malformed. Released binaries cannot be changed, so the RuntimePlan reference tells users to
give an older binary its own `ZEROSHOT_CONFIG_DIR` and to remove lanes with the newer binary to
recover.

## Runtime composition

### Candidate (`zeroshot/src/native_v2_candidate.rs`)

`NativeV2CandidateConfig.harness: NativeV2HarnessConfig` becomes
`lanes: Vec<NativeV2HarnessConfig>`. `NativeV2HarnessConfig` gains a `lane(&self) -> RuntimeLane`
method (Copilot reports `github`) and a workspace accessor.

`validate_config` builds a map from lane to config and rejects, in order:

| Condition                                                       | Error               |
| --------------------------------------------------------------- | ------------------- |
| Two configs report the same lane                                | `RuntimeMismatch`   |
| The configured lane set differs from `admitted.runtime.lanes()` | `RuntimeMismatch`   |
| Any config's workspace differs from the delivery workspace      | `WorkspaceMismatch` |

`RuntimeMismatch` keeps its variant and its mapping to `CapsuleAllocationUnavailable::Runtime`.
Its message becomes "candidate lanes do not match the admitted graph runtime". These are
composition invariants, not user input errors, so they do not need to name a lane.

`build_candidate` builds one adapter per config, local or hosted as today, into a
`BTreeMap<RuntimeLane, CandidateAgents>`.

`CandidateNodeLane` becomes:

```rust
struct CandidateNodeLane {
    default_lane: RuntimeLane,
    agents: BTreeMap<RuntimeLane, CandidateAgents>,
    delivery: Arc<NativeV2DeliveryAdapter>,
}
```

`open` and `run` resolve an agent binding's lane as `binding.lane.unwrap_or(default_lane)` and look
it up. A miss is `NodeRunnerError::Driver`; after `validate_config` it cannot happen, and there is
never a fallback to another lane.

Sessions need no change. The session pool is keyed by run and node instance, the private home by
role and node instance or execution, and each adapter downcasts only sessions it opened. Routing by
lane is consistent per node, so the downcast always matches.

### Local root (`zeroshot/src/native_v2_local.rs`)

`local_harness` becomes `local_lanes` and returns `Vec<NativeV2HarnessConfig>`. It computes the
shared inputs once (native environment snapshot, local command environment, search path, user
home, process pool) into a request struct, then builds one config per lane in
`admitted.runtime.lanes()`. The per-lane builder keeps today's three bodies unchanged, so each
Claude lane builds its own process environment from the shared search path and snapshot, and a
plan without a Claude lane never builds one. The four-parameter Clippy ceiling is respected
through the request struct.

### Hosted root (`zeroshot/src/native_v2_hosting/allocator.rs`)

`ProductionCapsuleAllocator::harness` becomes `lanes` with the same shape. The allocator config
already carries all three executables, validated non-empty at construction, and the capsule
filesystem is lane-agnostic.

### Portable controller and ACP

Both compose through the local root and need no composition change.

## Provider access (`zeroshot/src/native_v2_candidate/provider_access.rs`)

`provider_access_contract` takes a `RuntimeLane` instead of the plan. `materialize_provider_access`
reads the run-level lane once and, inside the loop that already visits every binding, derives the
contract from the binding's effective lane. The native-local exemption, authored-field precedence,
and connection fallback stay as they are, applied per node. Call sites at hosted submit, stored
remote profiles, and ACP are unchanged, and the hosted digest is still computed after
materialization. `RuntimePlan::connection_requirements` is already a union over bindings.

## Local preflight

Before the local CLI launches a detached controller, for `run` and for `resume` of a local run, it
checks every lane in the plan:

- Resolve the lane's executable name (`copilot`, `codex`, or `claude`) on the same search path the
  adapters will use: `PATH` from the captured shell snapshot, else the default search path.
- Fail with a usage error that names the lane and the executable, for example
  "lane claude/anthropic needs the `claude` executable on PATH", before any run state is created.

The lookup is `find_executable` in `zeroshot/src/execution/platform.rs`, beside the `executable`
resolution that process spawning uses, so preflight checks what spawning will run. On Windows,
`executable` spawns exactly its result: a bare name is tried only with the fixed suffixes `.exe`,
`.com`, `.cmd`, and `.bat`, in that order, in each directory of the child's `PATH`. `PATHEXT` is
ignored, and an extensionless file never matches. On other platforms spawning leaves the search to
the OS, which searches after the harness has changed into the directory it starts in, and
`find_executable` follows it from that directory: the run's workspace, or for `zeroshot acp`, the
session's workspace. A name containing a path separator is checked where it points from that
directory, and a bare name is looked up in each entry of `PATH` as a regular file with at least one
execute bit. Relative and empty entries resolve against that directory: an empty entry is the
directory itself, a relative entry is joined to it, and an absolute entry is used as it is.

The failure is `NativeV2CliError::Usage`, not the `NativeV2CliError::Local` that other local
errors use, so its message has no `local controller operation failed:` prefix and its diagnostic
code is `request.invalid`. The check is the first step of starting a local controller, before run
storage or the bootstrap exists, and a failed `resume` still reconciles the claimed workspace.
Adapters keep spawning by name, so turn behaviour is unchanged.

`zeroshot acp` has no session workspace at startup. After it validates its profile, it runs the
check against the directory it was started in only when every entry of the search path is
absolute, because then no directory can change the result; a failure is the same usage error.
Otherwise it skips the startup check. Every session runs the check when it opens, against that
session's workspace, before admission, and a missing executable fails `session/new` with ACP's
invalid-params error and the same message. On Windows the lookup ignores the directory, so the
deferred check gives the answer the startup check would have given, only later.

Preflight does not probe login state. The repository's invariants forbid validating provider
availability, and the three CLIs have no common headless status command. Connection values for
non-native lanes are already resolved for the union before launch. A login failure still surfaces
at the first turn on that lane, and that diagnostic already names the harness.

## ACP gate (`zeroshot/src/native_v2_cli/acp.rs`)

`validate_profile` replaces its run-level variant match with a check over every agent binding's
effective lane. Each must be a Codex or Claude lane; the message becomes "only Codex and Claude
lanes are supported".

## UI (`ui/src`)

- `domain.ts`: `Binding` gains `lane?: { harness: string; provider: string }`. Runtime validation
  requires a complete, schema-valid pair whenever `lane` is present and reports it like the
  run-level errors. The run-level change handler deletes `lane` from every node when the
  run-level harness changes and leaves nodes alone when only the provider changes.
- `Inspector.tsx`: agent nodes get harness and provider selects with a leading "Run default"
  option. Choosing a harness sets `lane` to that harness with an empty provider, and the provider
  options come from that harness's schema variant, as the run-level editor derives them today.
  Choosing "Run default" deletes `lane`. The model picker and model suggestions receive the node's
  effective lane.
- `RuntimeEditor.tsx`: the "Applies to the whole graph." copy becomes a note that the pair is the
  default for every node and that a node can override it in the inspector.
- `RunHistoryView.tsx`: when any node in a run's plan carries a lane, show each agent node's
  effective harness and provider beside its model.
- The served schema is `schema_for!(RuntimePlan)`, so the new field reaches the editor without
  UI-side schema changes.

`Binding` already keeps unknown keys through edits, so a UI build without this work preserves
lanes unchanged. The UI can land after the core.

## Docs and generated artifacts

- `docs/reference/runtime-plan.md`: replace "One harness, provider, and size apply to the whole
  run" with the default-lane wording, add the `lane` row to the agent table, add a "Per-node lane"
  section with the cross-vendor example from the issue in nested form, and say that provider
  access is derived per node from its effective lane.
- `docs/concepts/runtimes-and-connections.md`: materialization and local native login are per
  lane.
- `docs/concepts/targets.md`: a local run needs every lane's CLI installed, signed in for the
  native `codex`/`openai`, `claude`/`anthropic`, and `copilot`/`github` lanes; other lanes need
  their provider's connection values.
- `docs/guides/review-loop.md`: a short cross-vendor variant, with its runtime JSON added under
  `docs/assets/review-loop/`.
- `docs/guides/python-sdk.md`: one line that an opaque `RuntimePlan` may carry per-node lanes.
- `zeroshot/src/native_v2_cli/parser.rs`: `--runtime-config` help mentions per-node lanes; then
  regenerate `docs/zeroshot-cli.md` and `docs/zeroshot-cli.html` with the Rust example.
- `AGENTS.md`, runtime invariants: the run-level pair is the default lane, an agent node may
  carry a `lane` override, candidates build one adapter per distinct lane, and adapters never
  switch provider per turn. Targets advertise `openengine.node-runtime-lanes/v1`.
- Protocol tests: a lane-bearing submission fixture under
  `crates/openengine-cluster-protocol/tests/fixtures/` with a round-trip test.

## Error handling

- User-facing errors name what the user can act on: the lane and executable in preflight, the
  capability in the target check, the lane kind in the ACP gate.
- Composition mismatches stay internal invariant errors with the existing wire codes.
- There is no silent fallback to the run-level lane anywhere. A missing adapter is a driver error.
- Older targets are refused before submission. Older binaries reject the unknown field.

## Testing

Beside each owning module, following existing fixtures:

- Protocol: round trips with and without `lane`; byte-identical serialization without one;
  rejected invalid pairs; rejected `lane` on `git_delivery`; `lane`, `effective_lane`, `lanes`,
  and `nodes_mut`; discovery document with and without the marker; the regenerated schema passes
  `--check`.
- Candidate: two fake drivers, worker routed to one lane and reviewer to the other; the three
  validation failures; a node-instance session reused across a loop stays on its lane.
- Local and hosted roots: N configs for N lanes sharing the computed inputs; one config for a
  plan without overrides.
- Provider access: mixed lanes produce per-node contracts for local and contained placements;
  authored fields still win.
- Preflight: Unix executable fixtures created with
  `openengine_cluster_testkit::fixture::write_executable`; one missing lane executable yields the
  named error and no run state; all present proceeds; an empty or relative `PATH` entry finds an
  executable in the directory the harness starts in; on Windows only the fixed suffixes match.
- Target client: refusal without the marker and acceptance with it, for submission and remote
  profile set.
- ACP: mixed Codex and Claude accepted; a Copilot lane rejected. With an empty or relative `PATH`
  entry, startup succeeds without the CLI, and `session/new` fails with the lane message as invalid
  params until the CLI is reachable from the session's workspace.
- UI: vitest for the clearing rule, lane validation, inspector override and reset, and history
  display.
- Then the complete lanes from `CLAUDE.md`: fmt, clippy, test, doc, `npm run check`, format
  check, actionlint, the Python lane, and `python -m mkdocs build --strict`.

## Non-goals

- Per-node `size`.
- Provider selection per invocation inside one adapter.
- Overrides through `--uniform-runtime-config` or the Python `UniformRuntime`.
- Dropping or making optional the run-level harness and provider.
- Probing login state.
- Exposing lanes in status, watch, or observation records.
- Per-harness target images.

## Delivery shape

Two pull requests against `main`, each from an isolated worktree, each with a Conventional Commit
title and a `## Summary`:

1. `feat(runtime): choose the harness and provider per node in a RuntimePlan`: protocol types and
   schema, discovery marker and client check, candidate, both roots, provider access, preflight,
   ACP gate, docs, fixtures.
2. `feat(ui): edit per-node harness and provider lanes`: domain, inspector, runtime editor,
   history view, tests.
