---
description: 'The graph contract defines node types, control flow, state, signals and bounds for the full-graph and single-worker protocol profiles.'
---

# Graph contract

This document is normative together with the public Rust types in
`openengine-cluster-protocol`. The Rust types are authoritative when generated projections and
prose disagree. [Build a review loop](../../guides/review-loop.md) is a complete worked example.

The graph contract defines two profiles:

| Profile       | Wire identifier                     |
| ------------- | ----------------------------------- |
| Full graph    | `openengine.graph.full/v1`          |
| Single worker | `openengine.graph.single-worker/v1` |

`openengine-cluster-server::graph_verifier::ProductionGraphVerifier` verifies the full-graph
profile. It reads `GraphSpec`, resolves workers through `WorkerRegistry`, and returns
`CompiledGraphIr` with proven `StructuralBounds`. Parsing or schema validation alone does not admit
a graph; neither does round-tripping a fixture, constructing `CompiledGraphIr` directly, or hashing
its canonical form. The verifier does not store, schedule, or execute graphs. `ScriptedVerifier` is
only an admission fixture for tests. Protocol methods and advertised backend capabilities remain
separate from verification.

## Graph wire format

`GraphSpec` has the required camel-case fields `profile`, `initialInput`, `policy`, and `root`.
Every graph node is tagged by `kind`. Node payloads reject unknown fields.

| `kind`     | Contract                                                                                                                                                   |
| ---------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `step`     | Stable name and versioned worker reference; optional bounded authored instructions; declared input/output types and bindings; positive bounds              |
| `verifier` | The bounded worker surface plus optional bounded authored instructions, named finite-enum signals, and a declared diagnostic type                          |
| `seq`      | Declared state, non-empty ordered children, promoted state paths                                                                                           |
| `choice`   | Declared state, non-empty authored-order guarded branches, optional `otherwise`, promoted paths                                                            |
| `par`      | Declared state, non-empty branches, promoted paths, and a tagged join                                                                                      |
| `loop`     | Declared state, body, optional structured `until`, positive default `maxIterations`, optional `maxIterationsInput`, promoted paths; syntactically do-while |
| `map`      | Declared state, body, structured array selector `over`, positive concurrent-item limit `maxItems`, promoted paths                                          |
| `succeed`  | Declared output type and structured state-to-output bindings                                                                                               |
| `fail`     | Finite-enum reason; `unhandled` is reserved for a future compiler's implicit sink                                                                          |

Parallel joins are `all`, `any`, `quorum { count }`, or `first { when }`. The closed worker error
channel is `timeout`, `crash`, `malformed`, and `refusal`. Choice reachability/exhaustiveness, loop
exit satisfiability, selector typing, dominance, promotion safety, and structural folds are
production-verifier responsibilities, not parser claims. Omitting `timeoutMs` lets a node run until
completion or cancellation. An explicit `timeoutMs` accepts the `PositiveInteger` wire range.
Built-in graphs omit deadlines for every node; provider processes have no separate execution
deadline. Custom graphs may still declare a node deadline.
Normal-success continuation requires every branch for `all`, one branch for `any`/`first`, and
`count` branches for `quorum`. Quorum flow and promotion guarantees range over every valid
size-`count` completing branch set. A set is valid only when its branch-completion predicates are
jointly satisfiable under one legal control assignment; branches guarded by mutually exclusive
outcomes never fabricate a completion set. Quorum one behaves as alternatives, while quorum over
every branch carries the same completion guarantees as `all`. For `all`, `any`, and `quorum`,
`joined=reached|quorum_unreachable` is correlated with that required-completion predicate;
assignments combining `reached` with an unsatisfied predicate or `quorum_unreachable` with a
satisfied predicate are not legal control assignments. Inside a map, this correlation is applied
to each item's joint join/branch-control outcome before `k_of_map` aggregates label counts.
For `first`, a completed candidate uses its own available results and pre-parallel facts.
Other racers remain unknown. Short-circuit truth allows either accepted reviewer to establish
`any(a.accepted, b.accepted)`; conditional controls contribute only on their selected route.
The guard must reference a racer and at least one candidate must be able to establish it.
Viability follows the selected remaining flow: skipped producers cannot qualify a candidate, while
unfinished loop rounds may replace provisional controls. Future predicates share legal joint
outcomes, so one verdict cannot satisfy incompatible labels or both signal and error predicates.
Partial maps preserve settled item contributions while expanding future outcomes through the same
choice-presence and retained-branch correlations used at admission. Finishing an unrelated item tail
does not make a settled verdict replaceable. Loop revisits replace final-round controls, preserving
a known map cardinality when remaining visible writes cannot change its selected collection.
Selector projections use each suspended group's local state and respect write order and promotion
boundaries. A guaranteed loop exit prevents another round from replacing its settled controls.
Known map lengths require exactly that many item outcomes; a nonempty map can complete its racer
only when every selected body can finish normally under the same joint outcomes.
Nested aggregate folds preserve every enclosing map cardinality and full item index scope. Choice
conditions stay attached to future collection writes, and completion predicates share the actual
aggregate counts of other maps. Impossible joins retain settled prefix evidence during terminal
replay even when a disqualified racer still has pending work.
Map count predicates respect those cardinality constraints. Speculative oversized maps
can lose a race without triggering a work-limit failure; work that starts still consumes the
actual run ceilings.
Once no unfinished candidate can qualify, the join reports `no_satisfier` and stops unnecessary
work. Other impossible joins report `quorum_unreachable`; their queued eligible branches remain
potential contributors until control proves otherwise. Voided or unstarted losers provide no
controls or promoted writes.

Worker failures retain those four codes. Native machine-readable credential rejections use
`refusal` with reason `authentication_required`; rejected models use `refusal` with reason
`model_unavailable`, and their diagnostic names the requested model. A force-stopped node uses
`refusal` with reason `stopped` and logs that it was stopped. Guards on `refusal` see all three
cases as well as task and policy refusals. These outcomes receive neither an adapter continuation
nor another node attempt. Active Memory skips its postmortem, with an explanation in the log,
when the selected source node's final outcome is an authentication or model rejection, including
restored checkpoint prerequisites.

Classification uses each harness's native signals, without a Zeroshot model catalog or matching
human-readable error wording:

| Harness  | Rejected credential                                                                                                                         | Model not accepted                                                                                                                                                                                 |
| -------- | ------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Claude   | `authentication_failed` on `system/api_retry` or an assistant error, or numeric `api_error_status: 401`, followed by a failed result        | A failed result with the native stderr marker `[claude-code:unrecognized_model]` and its JSON `model` field. A successful result keeps its answer despite this warning.                            |
| Codex    | No distinct signal in exec JSON: `error` and `turn.failed.error` expose only `message`; remains `crash`/`declared_failure`.                 | Same message-only exec error; remains `crash`/`declared_failure`.                                                                                                                                  |
| Copilot  | `session.error.data.errorType: authentication`, including the equivalent request-correlated RPC error data                                  | Generic `query` errors do not distinguish model rejection; remains `crash`/`declared_failure` when no specific signal is available.                                                                |
| Pi       | Failed assistant messages expose `stopReason` and `errorMessage`, without a credential rejection class; remains `crash`/`declared_failure`. | Native model selection and failed assistant messages provide no distinct model rejection class; remains `crash`/`declared_failure`.                                                                |
| OpenCode | Native `ProviderAuthError`, or `APIError.data.statusCode: 401`                                                                              | Native `ProviderModelNotFoundError`, or `APIError.data.responseBody` containing JSON `error.code: model_not_found`. A bare 404 or a message-only startup error remains `crash`/`declared_failure`. |
| Hermes   | Failed native result `failure_reason: auth` or `auth_permanent`                                                                             | Failed native result `failure_reason: model_not_found` or `model_entitlement`. Older native results without `failure_reason` remain `crash`/`declared_failure` for either case.                    |

These mappings apply locally and through hosted capsule transport. A cut or malformed provider
response stream remains `crash`/`declared_failure` and retains the harness's existing continuation
policy and the node's crash attempts. A malformed **worker response contract** instead remains
`malformed`/`malformed_result` after its allowed corrections.

Claude account/session quota exhaustion uses `refusal` with the existing `declared_failure` reason
when a native `rate_limit_event` has `rate_limit_info.status: rejected` and the native turn ends
with an explicitly failed native result. Confirmed quota exhaustion bypasses adapter continuation
and automatic crash attempts. Missing, wrongly typed or unrecognized result subtypes cannot
confirm quota exhaustion; their existing failure handling stays in place. The ordinary
failure context identifies the affected node and retains available reset information plus manual
resume guidance. Whole-second numeric reset timestamps are displayed in UTC; other numeric hints
and ambiguous text such as `5:08 PM` retain their values without an inferred date or timezone.
Missing or malformed reset data does not hide a
confirmed quota failure. Allowed events, warnings (including `overageStatus: rejected` with an
allowed status), and rejected limits followed by a successful result do not fail a node. Ordinary
transient throttling without that confirmed rejection keeps its existing retry behavior.

Codex exec usage-limit failures remain `crash`/`declared_failure`: its native `error` and
`turn.failed.error` expose only `message`, including in
[Codex 0.166.0](https://github.com/openai/codex/blob/rust-v0.166.0/codex-rs/exec/src/exec_events.rs).
That interface has no machine-readable quota class or reset timestamp. Zeroshot does not classify
the message wording, so these failures retain continue-once and authored crash attempts. The
provider's reset hint remains in its diagnostic.

Built-in graphs take their existing failure route for a confirmed quota refusal. Custom guards
and loops retain control of recovery; there is no global abort, cooldown, waiting until reset, or
automatic resumption. After usage becomes available,
[resume the failed run](../../guides/observe-and-control.md#restart-or-resume-a-failed-run) using its
retained workspace or an available checkpoint.

## Data and control separation

No selector or guard is source text. `DataSelector` is tagged by `source` and reads only a bounded,
non-empty `FieldPath` from `state` or the current map `item`. An input binding has an explicit target
path and data selector. A write binding has an explicit target and a `NodeOutputSelector` naming a
node, the closed channel `out`, `signal`, or `diagnostic`, and a field path.

The graph format has no whole-payload binding: executable inputs and `succeed` outputs must be `null` or records
that bindings can construct through non-empty field paths. Scalar, enum, and array payloads remain
valid elsewhere in the closed algebra and as nested record fields; they are not implicitly copied
or defaulted at a payload root.

A root group can add required state fields with deterministic empty values: `null`, `""`, `[]`,
and records composed of such fields. Initial input is validated unchanged; these defaults belong
to graph state. Optional fields stay absent, and supplied values are preserved.

Control guards use a separate AST and cannot contain `DataSelector`:

| `kind`       | Fields                                                         |
| ------------ | -------------------------------------------------------------- |
| `in`         | One `ControlSelector` and a non-empty enum-label set           |
| `all`, `any` | Non-empty guard lists                                          |
| `not`        | One guard                                                      |
| `k_of_n`     | Positive count, non-empty finite selector list, enum-label set |
| `k_of_map`   | Positive count, one bounded group selector, enum-label set     |

A `ControlSelector` names a node/group, a closed source (`signal`, `error`, or `group`), and an
optional field. JavaScript, JSONPath, regular expressions, commands, endpoints, environment
variables, credentials, provider secrets, and free-form expressions are not fields in this AST.
Unknown-field rejection prevents them from being smuggled into otherwise valid nodes.

## Stable references and policies

Workers and policies are references of the form `name@positiveVersion`. Graphs cannot represent a
typed command, executable path, endpoint, credential, bearer token, environment value, inline
permission, or provider secret. Executable `instructions` are opaque prompt guidance and grant no
runtime authority. A policy binding contains one versioned policy reference and permits only
`deny` as its default. Registry descriptors and policy implementations are outside this contract.

## Closed payload algebra and subtyping

`PayloadType` is tagged by `kind` and is closed over `null`, `boolean`, `integer`, `number`,
`string`, structural `record`, homogeneous `array`, and finite `enum`. A record maps field names to
`{ type, required }`. Enum labels are non-empty, unique, sorted, and finite. The algebra contains no
unions, references, definitions, tuples, custom schema keywords, regex constraints, or arbitrary
JSON Schema.

For source type `S` and target type `T`, `S <: T` is exactly:

| Source and target            | Rule                             |
| ---------------------------- | -------------------------------- |
| Equal primitive kinds        | Yes                              |
| `integer` to `number`        | Yes                              |
| Any other primitive widening | No                               |
| `array<S>` to `array<T>`     | Exactly when `S <: T`            |
| Enum `S` to enum `T`         | Every label in `S` occurs in `T` |
| Record `S` to record `T`     | Width and depth rules below      |

For records, every required field of `T` must exist as required in `S`, and its value type must be a
subtype. An optional field of `T` may be absent from `S`; if present, its value must subtype the
target value. A required source field may satisfy an optional target field. An optional source field
never satisfies a required target field. Extra source fields are allowed. The relation is recursive,
deterministic, side-effect free, and never delegates to general JSON Schema evaluation.

## Production full-graph verification

The verifier runs deterministic synchronous structural and semantic passes before accessing the
worker registry. Any structural diagnostic prevents registry lookup. Worker absence, unavailable
versions, descriptor mismatch, profile mismatch, verifier-contract mismatch, and schema
incompatibility are graph rejections. Internal errors are reserved for impossible invariant or
compiled-IR construction failures after successful validation.

Selectors use closed finite domains: verifier signals use their declared labels; worker errors are
`timeout|crash|malformed|refusal`; loop `terminated` is `converged|exhausted`; `all|any|quorum` parallel `joined` is `reached|quorum_unreachable`; and `first`
parallel `raced` is `satisfied|no_satisfier`. Choices are first-true in authored order and are
checked over the bounded legal control space. An `otherwise` alternative is rejected as unreachable
when earlier branches cover that space and does not participate in flow analysis. Loops are
do-while. When present, their exit guard must use a verifier guaranteed to execute on every
iteration. When `until` is absent, the body repeats to `maxIterations` unless a terminal node ends
the graph first.
`maxIterationsInput`, when present, names an integer path in `initialInput`. Native-v2 run
admission replaces the default bound with that run's positive safe integer, then verifies the
resulting graph again. An absent input value retains the authored default. Reusable profiles are
verified against that default; the general execution and loop-entry budgets still constrain each
admitted run.
Signal and error controls from one executable are mutually exclusive outcomes. Map aggregates
count joint per-item outcomes, so one mapped execution cannot contribute both a success signal and
an error. They also preserve per-item execution flow: guaranteed sequential successors,
descendants of a full-completion parallel, and do-while body descendants must emit one success or
error outcome. A conditional successor emits an outcome exactly when its first-true choice residual
selects that route. For a promoted map path `p: array<T>`, a body write to `p` writes one `T` at the
current input index; indexed values are collected in input order. The promotion is total, so an
empty selected input defines `p` as `[]`, but the promoted array retains each mapped executable's
success/error provenance until control excludes every mapped runtime error. Choice residual
assignments determine channel availability:
`out`, `signal`, and `diagnostic` are unavailable on every reaching error path, while terminal
alternatives do not contaminate later fall-through flow. Every `k_of_n`/`k_of_map` label must belong
to the applicable closed selector domains. Executable write bindings are success-outcome effects:
an output-backed optional state path remains undefined while that executable's runtime-error
outcomes can reach the reader. A residual branch that excludes every runtime error makes the write
definite. State promotion preserves this outcome provenance and cannot turn a conditional write
into a definite one. Definition flow retains exact path/type guarantees from required initial input
through nested group-state widening. A descendant write invalidates stale ancestor refinements.
Parallel promotions remain conditional until a `reached` or `satisfied` residual excludes the
group failure outcome; unguarded continuation cannot consume success-only branch writes.
Conditional ownership is target-granular across nested parallel groups, choice merges, and later
sequential writers. Parallel failure labels `quorum_unreachable` and
`no_satisfier` restore the incoming pre-par definitions and expose no winner or branch-promotion
data. After successful routing, a binding defines its target only when the selected output or
diagnostic path is required; writing a required record defines only its required descendants, never
optional descendants.

The full-graph profile has fixed ceilings:

| Bound                         | Maximum |
| ----------------------------- | ------: |
| Graph nodes                   |   4,096 |
| Graph depth                   |      64 |
| Guard nodes                   |   4,096 |
| Assignments per finite check  |  65,536 |
| Concurrent map items          |  65,536 |
| Attempts per executable node  |     100 |
| One-run executable node count |  65,536 |
| Total loop-body entries       |  65,536 |
| Run execution concurrency     |  65,536 |

Completed-item replay retains scoped runtime facts and promoted values rather than full caller
state per item. Probe results are released before decision replay; collections still require every
item to write the promoted path and retain input ordering.

Maps process their complete input. `maxItems` limits the number of items whose complete bodies
are active, including nested work. Waiting parents consume no leaf execution slot. Runtime
`maxConcurrency` defaults to 64 on every OS and placement and accepts positive overrides through
65,536. It counts owned executing leaves and auxiliary agents; native tool subprocesses do not
consume separate slots. Queue waiting spends no attempt or execution timeout. Authored terminals
and joins can still void unnecessary work.

Only `step` and `verifier` count as logical executable visits. Retries stay in the same visit.
The host enforces 65,536 logical visits and 65,536 loop-body entries against durable history,
including checkpoint prefixes. Known excess work fails before dispatch; dynamic expansion fails
at the responsible node before crossing a ceiling. Concurrency never proves total cardinality.
Static graphs retain structural proofs; maps produce `workEnforcement: "runtime"` certificates.

Aggregate checks preserve each item's joint outcomes and conditional presence. Structural map
bodies can complete without executable visits, so analysis assumes no cardinality bound from
concurrency or leaf-work ceilings. The verifier clips
joint label counts at the largest queried threshold, so common `k_of_map(..., 1)` checks reach a
fixed set of states independent of input length. One verification admits at most 65,536 assignment
states and 8,388,608 count-cell operations. Each assignment transition charges its copied selector
and count cells before allocation. Excess analysis reports `ceiling_exceeded` at the relevant
node/guard; there is no user budget or timeout setting.

Graph-bearing JSON decoding first checks a fixed 512-container envelope for structural arrays,
payload nesting and transport wrappers. The graph verifier separately checks authored depth 64.

Nested parallel outcomes use joint retained-branch witnesses during analysis. The required winner
count and branch eligibility constrain these witnesses together, before per-item aggregation.
Conditional controls from omitted branches remain unavailable, including when a nested group was
skipped entirely. Proofs cannot combine results from mutually exclusive winners.

## Diagnostics, bounds, and artifact receipts

Verifier output uses structured `GraphDiagnostic` values. Severity and diagnostic code are
closed enums. Paths are arrays of field, index, or node segments, never JSONPath strings. The codes
cover schema safety, reachability, choice exhaustiveness, loop exit satisfiability, missing bounds,
write conflicts, ceiling excess, cyclic references, undefined reads, and invalid graph shape.

A rejected graph's error text names the first diagnostic's path and message, then the number of
further diagnostics. The path shows field names with their `[index]` and node names in backquotes.
The message is never shortened. The count and then the path only use the bytes the message leaves
within 1,024 bytes: the count is left out when it does not fit, and `.../` replaces leading path
segments that do not fit:

```text
graph verification rejected the graph at root/`run`/children[1]/`check`/branches: control assignment space exceeds 65536 (and 1 more diagnostic)
```

`StructuralBounds` distinguishes static and runtime work enforcement and carries a termination witness, positive maximum node executions, positive peak
concurrency, and positive per-node attempt ceilings. A termination witness is either an acyclic node
order or a bounded structural ranking with positive maximum iterations.

`ArtifactRef` is a durable receipt containing only:

- opaque `artifactId`;
- lowercase 64-hex `sha256`;
- JavaScript-safe `byteLength`;
- `mediaType` and stable `typeId`;
- producer node and worker reference;
- generation, run ID, and positive attempt lineage;
- `public`, `internal`, `confidential`, or `restricted` redaction class.

Artifact bytes, inline payloads, filesystem paths, signed URLs, bearer tokens, and credential
material are deliberately unrepresentable.

## Canonical compiled IR and identity

`CompiledGraphIr` contains the profile, initial input type, policy binding, root, and proven
structural bounds. Canonical bytes are compact UTF-8 JSON after these transformations:

- object/map keys, record fields, bindings, signal names, enum labels, policy/set-like collections,
  promoted paths, and other semantically unordered collections are sorted; binding and `k_of_n`
  selector multiplicity is preserved;
- commutative `all`/`any` operands are recursively flattened, sorted, and deduplicated;
- parallel branches are sorted by stable node name;
- sequence child order, choice branch order, loop/map body structure, and every semantic order are
  preserved;
- optional/default fields are serialized explicitly;
- floats and non-finite values are absent from and rejected by the closed IR algebra.

`GraphIdentity` is lowercase hexadecimal SHA-256 over exactly those canonical bytes. Equivalent IR
built with different map, set, guard, binding-order, or parallel insertion order has the same
identity. Changing semantic sequence order, authored instructions, a payload type, bound, worker
reference, policy reference, binding content or multiplicity, or `k_of_n` selector multiplicity
changes the identity.

## Generated conformance vectors

`graph.schema.json` is rooted at `GraphSpec`; `compiled-ir.schema.json` is rooted at
`CompiledGraphIr`. OpenRPC exposes these and the graph diagnostic, bounds, and artifact component
schemas, but defines no additional protocol method. Files under
`protocol/openengine-cluster/v1/fixtures/graph/` are deterministic syntax/canonicalization vectors.
Their Rust parser and JSON Schema checks are fixture validation only. Exact production-verifier
success and rejection envelopes live under `protocol/openengine-cluster/v1/fixtures/verifier/` and
are generated through the reusable server verifier.

Nested map guards count only item occurrences on selected choice routes. A queued item keeps
known immutable child-collection sizes, and settled aggregates from before a race remain
available to its completion guards. These facts let impossible races settle before unrelated
queued work starts.
