# Cluster Protocol v1 graph contract

This document is normative together with the public Rust types in
`openengine-cluster-protocol`. The Rust types are authoritative when generated projections and
prose disagree.

Cluster Protocol v1 defines two profiles:

| Profile       | Wire identifier                     |
| ------------- | ----------------------------------- |
| Full graph    | `openengine.graph.full/v1`          |
| Single worker | `openengine.graph.single-worker/v1` |

There is no production backend or production static verifier for the full-graph profile yet.
Parsing, schema validation, fixture round trips, construction of `CompiledGraphIr`, and canonical
hashing are not graph verification or admission. Only a future backend verifier may promote graph
syntax to admitted compiled IR. The protocol still implements only `initialize` and `get`, and
`initialize` advertises no capabilities.

## Graph wire format

`GraphSpec` has the required camel-case fields `profile`, `initialInput`, `policy`, and `root`.
Every graph node is tagged by `kind`. Node payloads reject unknown fields.

| `kind`     | Contract                                                                                                                                      |
| ---------- | --------------------------------------------------------------------------------------------------------------------------------------------- |
| `step`     | Stable name and versioned worker reference; declared input/output types; structured input/write bindings; positive `timeoutMs` and `attempts` |
| `verifier` | The bounded worker surface plus named finite-enum signals and a declared diagnostic type                                                      |
| `seq`      | Declared state, non-empty ordered children, promoted state paths                                                                              |
| `choice`   | Declared state, non-empty authored-order guarded branches, optional `otherwise`, promoted paths                                               |
| `par`      | Declared state, non-empty branches, promoted paths, and a tagged join                                                                         |
| `loop`     | Declared state, body, structured `until`, positive `maxIterations`, promoted paths; syntactically do-while                                    |
| `map`      | Declared state, body, structured array selector `over`, positive `maxItems`, promoted paths                                                   |
| `succeed`  | Declared output type and structured state-to-output bindings                                                                                  |
| `fail`     | Finite-enum reason; `unhandled` is reserved for a future compiler's implicit sink                                                             |

Parallel joins are `all`, `any`, `quorum { count }`, or `first { when }`. The closed worker error
channel is `timeout`, `crash`, `malformed`, and `refusal`. Choice reachability/exhaustiveness, loop
exit satisfiability, and selector type correctness are future-verifier responsibilities, not parser
claims.

## Data and control separation

No selector or guard is source text. `DataSelector` is tagged by `source` and reads only a bounded,
non-empty `FieldPath` from `state` or the current map `item`. An input binding has an explicit target
path and data selector. A write binding has an explicit target and a `NodeOutputSelector` naming a
node, the closed channel `out`, `signal`, or `diagnostic`, and a field path.

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
command, executable path, endpoint, credential, bearer token, environment value, inline permission,
or provider secret. A policy binding contains one versioned policy reference and the only v1
default, `deny`. Registry descriptors and policy implementations are outside this contract.

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

## Diagnostics, bounds, and artifact receipts

Future verifier output uses structured `GraphDiagnostic` values. Severity and diagnostic code are
closed enums. Paths are arrays of field, index, or node segments, never JSONPath strings. V1 codes
cover schema safety, reachability, choice exhaustiveness, loop exit satisfiability, missing bounds,
write conflicts, ceiling excess, cyclic references, undefined reads, and invalid graph shape.

`StructuralBounds` carries a termination witness, positive maximum node executions, positive peak
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
identity. Changing semantic sequence order, a payload type, bound, worker reference, policy
reference, binding content or multiplicity, or `k_of_n` selector multiplicity changes the identity.

## Generated conformance vectors

`graph.schema.json` is rooted at `GraphSpec`; `compiled-ir.schema.json` is rooted at
`CompiledGraphIr`. OpenRPC exposes these and the graph diagnostic, bounds, and artifact component
schemas, but adds no future protocol method. Files under
`protocol/openengine-cluster/v1/fixtures/graph/` are deterministic conformance vectors. Their Rust
parser and JSON Schema checks are fixture validation only and must not be described as the native or
production verifier for `openengine.graph.full/v1`.
