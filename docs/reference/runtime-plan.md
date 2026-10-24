# RuntimePlan

A `RuntimePlan` says how each executable node in a graph runs. It selects the run size and a default
harness and provider, and gives every `step` and `verifier` one binding. It contains model
identifiers and connection field names, never secret values.

[Runtimes and connections](../concepts/runtimes-and-connections.md) explains the concepts, and
[Build a review loop](../guides/review-loop.md) builds a complete plan for a custom graph. The Rust
types in `openengine-cluster-protocol` are authoritative; the
<a href="../cluster/schema.json#/$defs/RuntimePlan">generated protocol schema</a> is the
machine-readable contract.

## Document fields

`RuntimePlan` is a closed JSON object. The following four fields are required; unknown fields are rejected.

| Field      | Value                                                                              |
| ---------- | ---------------------------------------------------------------------------------- |
| `harness`  | Harness of the default lane: `codex`, `claude`, `copilot`, or `pi`                 |
| `provider` | Provider of the default lane, supported by the selected harness                    |
| `size`     | `small`, `medium`, or `large`                                                      |
| `nodes`    | Object mapping each executable graph node name to a [node binding](#node-bindings) |

Optional `activeMemory` controls repository and graph recall/collection independently, and
`globalAgents.postmortem` configures the built-in after-run specialists. Both default to enabled
on local CLI runs and direct targets. The same sections are accepted by uniform runtime files.
See [Active Memory](../guides/active-memory.md) for budgets, provider inheritance and opt-out examples.

`harness` and `provider` form the default lane. Every agent node runs on it unless its binding
carries its own [`lane`](#per-node-lanes). `size` applies to the whole run. Model, effort, session
scope, and connections are chosen per node.

`size` is a run-level resource class. Zeroshot records it with the run and reports it in run
status; the hosting target decides what resources each class receives.

`nodes` keys are graph node names: 1 to 128 bytes matching `[A-Za-z_][A-Za-z0-9_.-]*`.

This plan binds the three agent nodes of the built-in `software-change` template:

```json
{
  "harness": "claude",
  "provider": "anthropic",
  "size": "medium",
  "nodes": {
    "worker": { "kind": "agent", "model": "PROVIDER_MODEL_ID", "sessionScope": "node_instance" },
    "acceptance": { "kind": "agent", "model": "PROVIDER_MODEL_ID", "effort": "high" },
    "code": { "kind": "agent", "model": "PROVIDER_MODEL_ID", "effort": "high" }
  }
}
```

## Harness and provider

`harness` selects which `provider` values parse:

| `harness` | `provider`                                                |
| --------- | --------------------------------------------------------- |
| `codex`   | `openai`, `openrouter`, `bedrock`, `gateway`              |
| `claude`  | `anthropic`, `openrouter`, `bedrock`, `gateway`           |
| `copilot` | `github`                                                  |
| `pi`      | `anthropic`, `openai`, `openrouter`, `bedrock`, `gateway` |

The same pairs apply to an agent node's [`lane`](#per-node-lanes). Any other pair is rejected when
the plan is read.

`pi/anthropic` and `pi/openai` reuse a local Pi login from `~/.pi/agent`; the other `pi` lanes
need a connection.

`pi/gateway` carries `GATEWAY_BASE_URL`, `GATEWAY_API_KEY`, and `GATEWAY_API`. The last names the
wire protocol as `openai-responses`, `openai-completions`, or `anthropic-messages`, so one lane
reaches any endpoint Pi implements and the model identifier stays opaque and caller-owned. Zeroshot
never infers the protocol from the identifier, and an unrecognized value fails before launch.

To send the `anthropic` lane to a caller-owned Messages-compatible endpoint, declare
`ANTHROPIC_BASE_URL` with any one of the lane's credentials (`ANTHROPIC_API_KEY`,
`ANTHROPIC_AUTH_TOKEN`, or `ANTHROPIC_OAUTH_TOKEN`); a local run also honors the invoking shell's
`ANTHROPIC_BASE_URL`. The endpoint override interpolates the API key only when the connection
supplies it, so a token keeps Pi's own environment authentication. The `openai` lane likewise
honors `OPENAI_BASE_URL` or `OPENAI_API_BASE`. Pi ignores these variables itself, so Zeroshot turns each into a provider
override that keeps that provider's own protocol; the `openai` lane therefore needs a
Responses-compatible endpoint, and any other protocol uses `pi/gateway`. A declared endpoint ends
native-login reuse, so the stored login is never sent to that host.

Pi has no permission control of its own, so a contained `pi` run relies on the disposable capsule
rather than a bypass argument.

## Node bindings

`kind` selects an agent, Git delivery, or comment delivery binding. Fields belonging to another
kind are rejected.

### `agent`

| Field          | Required | Value                                                              |
| -------------- | -------- | ------------------------------------------------------------------ |
| `kind`         | Yes      | `agent`                                                            |
| `lane`         | No       | [Harness and provider](#per-node-lanes); default lane when omitted |
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

A `git_delivery` binding has no model, effort, session scope, or lane. Admission accepts it only on
a `verifier` node whose worker is one of the built-in delivery workers:

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

### `comment_delivery`

This binding accepts only `kind: "comment_delivery"` and optional `connections`. It has no model,
lane, effort, or provider session. It binds a verifier using `builtin.comment-delivery.post@1`
with that worker's fixed input/output, signal, and diagnostic contracts and no authored instructions.

The node reads a required discussion `url` and report `body`, plus optional captured `reviewScope`.
Its URL and present scope bind directly from required caller input; they cannot change before
publication. A graph permits one trusted delivery node, and comment publication must follow all
other executable work and cleanup, outside loops and maps. The exact confirmed receipt must appear
in terminal output. See [Review an issue or pull request](../guides/review.md).

## Per-node lanes

A lane is one harness and provider pair. The plan's `harness` and `provider` are the default lane
for every agent node, and an `agent` binding can carry its own `lane`. A node's effective lane is
its `lane`, or the default lane when it has none. `size` stays run-level.

This plan binds the same three nodes as the [first example](#document-fields). `worker` runs
on the default `codex`/`openai` lane, and both reviewers run on Claude Code:

```json
{
  "harness": "codex",
  "provider": "openai",
  "size": "medium",
  "nodes": {
    "worker": { "kind": "agent", "model": "PROVIDER_MODEL_ID", "sessionScope": "node_instance" },
    "acceptance": {
      "kind": "agent",
      "lane": { "harness": "claude", "provider": "anthropic" },
      "model": "PROVIDER_MODEL_ID",
      "effort": "high"
    },
    "code": {
      "kind": "agent",
      "lane": { "harness": "claude", "provider": "anthropic" },
      "model": "PROVIDER_MODEL_ID",
      "effort": "high"
    }
  }
}
```

Each `model` goes to the provider of its node's effective lane, so here `worker` needs an OpenAI
model identifier and the reviewers need Anthropic ones.

- `lane` accepts the same pairs as the run level, listed in
  [Harness and provider](#harness-and-provider). An invalid pair is rejected when the plan is read.
- A `git_delivery` or `comment_delivery` binding cannot carry a `lane`.
- A `lane` equal to the run-level pair is accepted, and the node runs exactly as it would without
  one. It is still a `lane` field, so the [target support](#target-support) check applies to it and
  older binaries reject it. Omit `lane` when a node should use the default lane.

Zeroshot starts one harness adapter for each distinct effective lane, and each node runs on its own
lane's CLI. Model, effort, session scope, and connections stay per node. Provider access defaults
also come from each node's effective lane; see [Connections](#connections).

### Target support

Targets that support lanes advertise `openengine.node-runtime-lanes/v1` in their discovery
document. Before the CLI submits a run or stores a profile on a target, it checks the plan. If any
node carries a `lane` and the target does not advertise the capability, the command fails after
reading the target's discovery document and before it requests an access token or sends the plan:

```text
target operation failed: target does not support per-node runtime lanes (openengine.node-runtime-lanes/v1)
```

Plans without a `lane` are unchanged on the wire and work with every target. Older Zeroshot
binaries reject a plan that contains a `lane` field when they read it.

Local profiles are saved together in one file in the configuration store (`ZEROSHOT_CONFIG_DIR`).
Once any local profile carries a `lane`, an older binary cannot read that file at all. Every local
profile command, `--profile local:NAME` run, and `zeroshot acp` session then fails, including for
profiles without lanes:

```text
local controller operation failed: local profile store is malformed
```

To run an older binary beside a newer one, give it its own `ZEROSHOT_CONFIG_DIR`. To recover, remove
the profile or its `lane` fields with the newer binary.

### Local runs

Before the controller starts, a local `zeroshot run` or `zeroshot resume` checks that each lane's
executable, `codex`, `claude`, `copilot`, or `pi`, is on `PATH`. A missing executable fails the
command with a message such as:

```text
lane claude/anthropic needs the `claude` executable on PATH
```

`zeroshot acp` runs the same check when each session opens, against that session's workspace, and
the session request fails with the same message. When every `PATH` entry is absolute, it also
checks at startup. An empty or relative entry resolves against the directory the harness starts in,
which is known only per session, so then the startup check is skipped. The check does not inspect login state; a missing login surfaces at
that lane's first turn. As in a single-lane run, local `codex`/`openai`, `claude`/`anthropic`,
`copilot`/`github`, `pi`/`anthropic`, and `pi`/`openai` lanes reuse their harness's native login,
and other lanes need their provider's connection fields; see [Connections](#connections).

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

The example below is the shape a Pi `gateway` lane uses: `GATEWAY_API` names its wire protocol and
is Pi-specific. Other lanes list only their own provider's fields.

```json
{
  "connections": {
    "gateway": ["GATEWAY_BASE_URL", "GATEWAY_API_KEY", "GATEWAY_API"]
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

When a node omits `connections`, Zeroshot derives provider access from that node's
[effective lane](#per-node-lanes) for the execution placement. Local native logins need no
declaration; contained targets and non-native providers receive the canonical requirements listed in
[Runtimes and connections](../concepts/runtimes-and-connections.md#runtime-configuration-contains-names-not-secret-values).
A run's connection requirements are the union over its nodes, so a plan that mixes lanes can require
the fields of several providers.

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
worker. Those get a `git_delivery` binding as described below.

When selecting the built-in `software-change` template, omitted `sessionScope` defaults to
`node_instance` for `worker` and `execution` for its reviewers. The worker retains its session
through implementation and every repair; reviewers start fresh each pass. An explicit uniform
`sessionScope` still applies to every agent. Exact runtime plans and saved graphs retain their
authored scopes; use `node_instance` on `worker` in an exact plan to retain its session.
When using the updated template with an older exact runtime config, remove its `review_repair`
and `delivery_repair` bindings. Existing saved profiles keep their stored graph and bindings.

For `auto-research`, an omitted uniform `sessionScope` defaults to `node_instance` for `explorer`,
`synthesizer`, `challenger`, and `researcher`, and `execution` for `reviewer`. The normal round has
these five agent bindings; `--push` adds `checkpoint_delivery`. Exact plans must supply these node
names and their desired scopes. Existing saved profiles keep their stored topology and runtime.

A uniform configuration applies one lane to every agent node. Neither `--uniform-runtime-config` nor
the Python SDK's `UniformRuntime` can express [per-node lanes](#per-node-lanes); use an exact
`--runtime-config` or an opaque `RuntimePlan` instead.

## Template delivery bindings

Built-in templates own their delivery nodes. `software-change` defaults to a pull-request delivery
verifier named `deliver`; `--push` and `--ship` select push and merge delivery, and `--delivery none`
omits delivery. The existing `worker` handles delivery repairs. With
`auto-research`, `--push` adds only the trusted `checkpoint_delivery` node; the researcher and
reviewer already handle the round records and delivery description.
`single-worker` has no delivery modes.

The CLI inserts a `git_delivery` binding with no forge connection for push, or
`{"github": ["GH_TOKEN"]}` for PR and merge. Git transport uses the selected
[source credentials](../concepts/targets.md#source-and-delivery-credentials).
`--no-pr-feedback` (requires pull-request or merge delivery) sets `pullRequestFeedback` to `ignore`.

A uniform configuration binds the added agent nodes too. An exact `--runtime-config` must bind
all agent nodes itself. Software-change uses `worker`, `acceptance`, and `code` with every delivery
mode. It may omit the delivery node; if it includes it, the binding must be `git_delivery`. List the executable nodes
with `zeroshot template show TEMPLATE` plus the same delivery flag.

Custom graphs author delivery in both the graph and the plan.
