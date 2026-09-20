# Use a local graph as an ACP agent

`zeroshot acp` is an experimental stdio endpoint that presents one saved local profile as an
Agent Client Protocol (ACP) agent. Each ACP prompt runs the complete graph once, while the ACP
session keeps the Git workspace and node provider sessions alive between prompts.

## Profile contract

The ACP preview accepts a deliberately narrow profile:

- The graph input is one required string field named `task`.
- Every success node returns one required string field named `response`.
- Map nodes and Git delivery nodes aren't accepted.
- Every executable node has an agent runtime binding with `sessionScope` set to `node_instance`.
- Runtime connections aren't accepted. The endpoint supports Codex and Claude through their local
  user login and configuration.

These limits keep one ACP turn equivalent to one ordinary local run; the graph compiler, reducer,
provider adapters, and run ledger stay unchanged.

Save the graph and runtime as a local profile:

```console
zeroshot profile set acp-worker \
  --graph graph.json \
  --runtime-config runtime.json
```

A minimal runtime has one node binding:

```json
{
  "harness": "codex",
  "provider": "openai",
  "size": "medium",
  "nodes": {
    "worker": {
      "kind": "agent",
      "model": "PROVIDER_MODEL_ID",
      "sessionScope": "node_instance"
    }
  }
}
```

The corresponding graph has a `task` input, writes the worker's response into state, then returns
it from the success node:

```json
{
  "profile": "openengine.graph.full/v1",
  "initialInput": {
    "kind": "record",
    "fields": {
      "task": { "type": { "kind": "string" }, "required": true }
    }
  },
  "policy": { "policy": "policy.native-v2@1", "default": "deny" },
  "root": {
    "kind": "seq",
    "name": "root",
    "state": {
      "kind": "record",
      "fields": {
        "task": { "type": { "kind": "string" }, "required": true },
        "response": { "type": { "kind": "string" }, "required": true }
      }
    },
    "children": [
      {
        "kind": "step",
        "name": "worker",
        "worker": "agent.worker@1",
        "instructions": "Complete the task and return a concise response.",
        "input": {
          "kind": "record",
          "fields": {
            "task": { "type": { "kind": "string" }, "required": true }
          }
        },
        "output": {
          "kind": "record",
          "fields": {
            "response": { "type": { "kind": "string" }, "required": true }
          }
        },
        "inputBindings": [
          {
            "target": ["task"],
            "value": { "source": "state", "path": ["task"] }
          }
        ],
        "writeBindings": [
          {
            "target": ["response"],
            "value": {
              "node": "worker",
              "channel": "out",
              "path": ["response"]
            }
          }
        ],
        "attempts": 1
      },
      {
        "kind": "succeed",
        "name": "done",
        "output": {
          "kind": "record",
          "fields": {
            "response": { "type": { "kind": "string" }, "required": true }
          }
        },
        "bindings": [
          {
            "target": ["response"],
            "value": { "source": "state", "path": ["response"] }
          }
        ]
      }
    ],
    "promotedStatePaths": []
  }
}
```

## Start the endpoint

Configure the ACP client to start this command in the canonical root of the Git worktree:

```console
zeroshot acp --profile local:acp-worker
```

ACP owns standard input and standard output for the lifetime of the process. Diagnostics go to
standard error. The process accepts one active ACP session, doesn't implement session loading, and
rejects MCP servers or extra workspace directories.

The client sends one nonempty ACP text block per prompt. Zeroshot maps it to `{"task":"..."}` and
streams the successful `response` as an agent message. The prompt response includes
Zeroshot-specific metadata with the durable run ID and raw graph output:

```json
{
  "zeroshot": {
    "runId": "01...",
    "rawOutput": { "response": "..." }
  }
}
```

`rawOutput` is Zeroshot's JSON form of the graph's terminal result, before ACP turns it into an
agent message. Success is `{"response":"..."}`; an authored fail node is `{"failed":"REASON"}`.

Use that run ID with the ordinary read commands:

```console
zeroshot status RUN_ID
zeroshot logs RUN_ID
```

Each prompt also appears in `zeroshot ui` as a normal local run, including its graph and retained
history.

## Session and run lifetime

The ACP session takes one workspace lease and pins the workspace's filesystem identity. Losing
either one stops the active turn as `runtime_lost` and rejects later prompts. Each prompt gets a
fresh run ID, controller lock, SQLite ledger, and supervisor lifecycle, so it remains visible after
the ACP session closes.

Provider sessions use stable graph-node slots within the outer ACP session. A clean node completion
can therefore resume the same Codex thread or Claude session on the next prompt.
Closing the ACP session cancels an active prompt, waits for it to settle, closes every provider
session, and releases the workspace lease. `session/cancel` force-stops only the current durable
turn and returns ACP's `cancelled` stop reason.

Authored graph failures are normal turn results: the agent emits a failure message and records the
failure under `rawOutput`. Admission, storage, or supervisor faults return a generic ACP internal
error and write the safe diagnostic to standard error.
