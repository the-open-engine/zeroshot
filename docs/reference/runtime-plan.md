# RuntimePlan

A `RuntimePlan` says how each executable node in a graph runs. It selects the harness, provider, and
run size, and gives every `step` and `verifier` one binding. It contains model identifiers and
connection field names, never secret values.

[Runtimes and connections](../concepts/runtimes-and-connections.md) explains the concepts, and
[Build a review loop](../guides/review-loop.md) builds a complete plan for a custom graph. The Rust
types in `openengine-cluster-protocol` are authoritative; the
<a href="../cluster/schema.json#/$defs/RuntimePlan">generated protocol schema</a> is the
machine-readable contract.

## Document fields

`RuntimePlan` is a closed JSON object. All four fields are required, and unknown fields are rejected.

| Field      | Value                                                                              |
| ---------- | ---------------------------------------------------------------------------------- |
| `harness`  | `codex`, `claude`, or `copilot`                                                    |
| `provider` | A provider supported by the selected harness                                       |
| `size`     | `small`, `medium`, or `large`                                                      |
| `nodes`    | Object mapping each executable graph node name to a [node binding](#node-bindings) |

One harness, provider, and size apply to the whole run. Model, effort, session scope, and
connections are chosen per node.

`size` is a run-level resource class. Zeroshot records it with the run and reports it in run
status; the hosting target decides what resources each class receives.

`nodes` keys are graph node names: 1 to 128 bytes matching `[A-Za-z_][A-Za-z0-9_.-]*`.

This plan binds the four agent nodes of the built-in `software-change` template:

```json
{
  "harness": "claude",
  "provider": "anthropic",
  "size": "medium",
  "nodes": {
    "worker": { "kind": "agent", "model": "PROVIDER_MODEL_ID", "sessionScope": "node_instance" },
    "acceptance": { "kind": "agent", "model": "PROVIDER_MODEL_ID", "effort": "high" },
    "code": { "kind": "agent", "model": "PROVIDER_MODEL_ID", "effort": "high" },
    "review_repair": { "kind": "agent", "model": "PROVIDER_MODEL_ID" }
  }
}
```

## Harness and provider

`harness` selects which `provider` values parse:

| `harness` | `provider`                                      |
| --------- | ----------------------------------------------- |
| `codex`   | `openai`, `openrouter`, `bedrock`, `gateway`    |
| `claude`  | `anthropic`, `openrouter`, `bedrock`, `gateway` |
| `copilot` | `github`                                        |

Any other pair is rejected when the plan is read.

## Node bindings

`kind` selects one of two binding shapes. Fields that belong to the other kind are rejected.

### `agent`

| Field          | Required | Value                                                              |
| -------------- | -------- | ------------------------------------------------------------------ |
| `kind`         | Yes      | `agent`                                                            |
| `model`        | Yes      | Provider-owned model identifier, 1 to 2,048 non-control characters |
| `effort`       | No       | `low`, `medium`, `high`, `xhigh`, or `max`; unset when omitted     |
| `sessionScope` | No       | `execution` (default) or `node_instance`                           |
| `connections`  | No       | [Connection declarations](#connections); empty when omitted        |

Zeroshot passes `model` and `effort` to the harness unchanged. It keeps no model catalog and does not
check whether a model supports the chosen effort.

`sessionScope` controls provider conversation state. It does not change graph state or bindings.

| `sessionScope`  | Behavior                                                                                      |
| --------------- | --------------------------------------------------------------------------------------------- |
| `execution`     | Opens a fresh provider session for each execution of the node, then closes it                 |
| `node_instance` | Reuses one session when the same node instance runs again, for example across loop iterations |

If a reused session is lost, Zeroshot fails that execution with `session_lost`. It does not open a
replacement session silently.

### `git_delivery`

| Field                 | Required | Value                                                       |
| --------------------- | -------- | ----------------------------------------------------------- |
| `kind`                | Yes      | `git_delivery`                                              |
| `connections`         | No       | [Connection declarations](#connections); empty when omitted |
| `pullRequestFeedback` | No       | `consider` (default) or `ignore`                            |

A `git_delivery` binding has no model, effort, or session scope. Admission accepts it only on a
`verifier` node whose worker is one of the built-in delivery workers:

| Worker                         | Accepts `pullRequestFeedback: ignore` |
| ------------------------------ | ------------------------------------- |
| `builtin.git-delivery.push@1`  | No                                    |
| `builtin.git-delivery.pr@1`    | No                                    |
| `builtin.git-delivery.pr@2`    | Yes                                   |
| `builtin.git-delivery.merge@1` | No                                    |
| `builtin.git-delivery.merge@2` | No                                    |
| `builtin.git-delivery.merge@3` | Yes                                   |

The delivery verifier cannot have authored `instructions`, must use `attempts: 1`, and must declare
the output, signals, and diagnostic of its worker's fixed contract. `zeroshot template show
software-change --pr` prints a delivery node with the current contract.

A graph can contain at most one delivery node, and no other executable node may run in parallel with
it, either as a `par` sibling or inside a `map` that can have more than one item. A hosting service
may require exactly one delivery node; Zeroshot Cloud quick runs require one. See
[concurrent workspace changes](../concepts/execution.md#concurrent-workspace-changes).

`ignore` skips pull-request discussion. CI, conflict, freshness, and merge-policy checks still apply.

## Coverage

`nodes` contains exactly one binding for every `step` and `verifier` in the graph, keyed by the node
name. Structural nodes (`seq`, `choice`, `par`, `loop`, `map`) and terminal nodes (`succeed`,
`fail`) never get a binding. Admission rejects:

- a missing binding for an executable node;
- a binding for a structural, terminal, or unknown node;
- a `git_delivery` binding on a `step`, or on a verifier that does not use a delivery worker;
- an `agent` binding on a node that uses a delivery worker;
- an `agent` binding on a node without authored `instructions`;
- a worker reference reused on several nodes with a different contract or binding kind.

## Connections

`connections` maps a connection key to the exact environment field names the node may receive.
Values come from the submission environment or a target connection store at run time.

```json
{
  "connections": {
    "gateway": ["GATEWAY_BASE_URL", "GATEWAY_API_KEY"]
  }
}
```

| Item                       | Limit                                                                  |
| -------------------------- | ---------------------------------------------------------------------- |
| Connection key             | 1 to 128 bytes, no control characters                                  |
| Field name                 | `[A-Za-z_][A-Za-z0-9_]*`, at most 128 bytes                            |
| Field list                 | At least one name, no duplicates                                       |
| Keys per node              | At most 64                                                             |
| Field names per node       | At most 64, and one name cannot appear under two keys in the same node |
| Unique field names per run | At most 64 across all nodes                                            |

The per-run limit also counts the connection fields and variable names of an attached runtime
environment. Each connection value must be 1 byte to 64 KiB with no NUL. The static connection
values supplied for one run, counting each key and field name with its value, must fit in 256 KiB.

When a node omits `connections`, Zeroshot derives provider access for the execution placement.
Local native logins need no declaration; contained targets and non-native providers receive the
canonical requirements listed in
[Runtimes and connections](../concepts/runtimes-and-connections.md#runtime-configuration-contains-names-not-secret-values).

## Uniform runtime configuration

`--uniform-runtime-config` accepts a single agent configuration and expands it into a full plan:

| Field          | Required | Value                                 |
| -------------- | -------- | ------------------------------------- |
| `harness`      | Yes      | As in `RuntimePlan`                   |
| `provider`     | Yes      | As in `RuntimePlan`                   |
| `size`         | No       | As in `RuntimePlan`; default `medium` |
| `model`        | Yes      | As in an `agent` binding              |
| `effort`       | No       | As in an `agent` binding              |
| `sessionScope` | No       | As in an `agent` binding              |
| `connections`  | No       | Applied to every agent node           |

Every executable node gets an `agent` binding with these values, except nodes that use a delivery
worker. Those get a `git_delivery` binding that declares `{"github": ["GH_TOKEN"]}`.

## Template delivery bindings

Built-in templates own their delivery nodes. With `software-change`, `--push`, `--pr`, and `--ship`
add a delivery verifier named `deliver` and a `delivery_repair` step. With `auto-research`, `--push`
adds `checkpoint_delivery` plus the agent nodes `checkpoint_manifest` and `checkpoint_audit`.
`single-worker` has no delivery modes.

The CLI inserts the template-owned `git_delivery` binding for the delivery node, declaring
`{"github": ["GH_TOKEN"]}`. `--no-pr-feedback` (requires `--pr` or `--ship`) sets its
`pullRequestFeedback` to `ignore`.

A uniform configuration binds the added agent nodes too. An exact `--runtime-config` must bind them
as `agent` nodes itself, such as `delivery_repair` for `software-change --pr`. It may omit the
delivery node; if it includes it, the binding must be `git_delivery`. List the executable nodes
with `zeroshot template show TEMPLATE` plus the same delivery flag.

Custom graphs author delivery in both the graph and the plan.
