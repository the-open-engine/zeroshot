---
description: 'Pause a graph until a time or for a duration with the built-in wait verifier, and run long work over many items in batches.'
---

# Wait until a time

A graph can pause until a time or for a duration with `builtin.request.wait@1`. It is a
host-run verifier, not a node `kind`: declare it as a `verifier` with that worker and a
`{"kind":"request"}` runtime binding. When the time passes, it emits signal `wait` with label
`elapsed` and the graph continues.

This graph waits until `dueAt`, then runs one worker pass:

```json title="graph.json"
{
  "profile": "openengine.graph.full/v1",
  "initialInput": {
    "kind": "record",
    "fields": {
      "task": {
        "required": true,
        "type": {
          "kind": "string"
        }
      },
      "dueAt": {
        "required": true,
        "type": {
          "kind": "string"
        }
      }
    }
  },
  "policy": {
    "policy": "policy.native-v2@1",
    "default": "deny"
  },
  "root": {
    "kind": "seq",
    "name": "root",
    "state": {
      "kind": "record",
      "fields": {
        "task": {
          "required": true,
          "type": {
            "kind": "string"
          }
        },
        "dueAt": {
          "required": true,
          "type": {
            "kind": "string"
          }
        }
      }
    },
    "children": [
      {
        "kind": "verifier",
        "name": "pause",
        "worker": "builtin.request.wait@1",
        "input": {
          "kind": "record",
          "fields": {
            "dueAt": {
              "required": false,
              "type": {
                "kind": "string"
              }
            },
            "durationMs": {
              "required": false,
              "type": {
                "kind": "integer"
              }
            }
          }
        },
        "output": {
          "kind": "null"
        },
        "signals": {
          "wait": ["elapsed"]
        },
        "diagnostic": {
          "kind": "record",
          "fields": {
            "message": {
              "required": true,
              "type": {
                "kind": "string"
              }
            }
          }
        },
        "inputBindings": [
          {
            "target": ["dueAt"],
            "value": {
              "source": "state",
              "path": ["dueAt"]
            }
          }
        ],
        "writeBindings": [],
        "attempts": 1
      },
      {
        "kind": "step",
        "name": "worker",
        "worker": "agent.worker@1",
        "instructions": "Do the task in the shared repository and run focused checks. Return null.",
        "input": {
          "kind": "record",
          "fields": {
            "task": {
              "required": true,
              "type": {
                "kind": "string"
              }
            }
          }
        },
        "output": {
          "kind": "null"
        },
        "inputBindings": [
          {
            "target": ["task"],
            "value": {
              "source": "state",
              "path": ["task"]
            }
          }
        ],
        "writeBindings": [],
        "attempts": 1
      },
      {
        "kind": "succeed",
        "name": "done",
        "output": {
          "kind": "null"
        },
        "bindings": []
      }
    ],
    "promotedStatePaths": []
  }
}
```

```json title="runtime.json"
{
  "harness": "codex",
  "provider": "openai",
  "nodes": {
    "worker": {
      "kind": "agent",
      "model": "gpt-5.3-codex",
      "sessionScope": "execution",
      "connections": {}
    },
    "pause": {
      "kind": "request"
    }
  }
}
```

```json title="input.json"
{
  "task": "Check whether the flaky search test still fails.",
  "dueAt": "2026-12-01T09:00:00+01:00"
}
```

Validate the files, then start the graph in your repository:

```bash
zeroshot run --title "Recheck the flaky search test" --graph graph.json --input input.json --runtime-config runtime.json --validate-only
zeroshot run --title "Recheck the flaky search test" --graph graph.json --input input.json --runtime-config runtime.json
```

## The wait contract

The input schema, output, signal and diagnostic are fixed, as in the example. Bind exactly one of
`dueAt` (an RFC 3339 time with an offset) or `durationMs` (an integer) from graph state with
ordinary input bindings. A time in the past or a nonpositive duration elapses immediately. Missing
or both fields, a time without an offset and a deadline more than 30 days ahead end the node with
`refusal` and a log explanation; route that error with a guard or the path fails as `unhandled`.
The node has one attempt and no instructions or `timeoutMs`.

A wait may appear in sequences, choices and loops, including inside a loop body, but never below
a parallel or map group. Stack profiles cannot contain waits.

Local and direct targets wait in place. Waits use wall time and recheck within one minute after the
machine sleeps. A host may set an in-place threshold: a longer wait suspends the run at its
checkpoint with reason `wait` and `wait.dueAt`, and the controller exits. `zeroshot suspend RUN_ID`
during a wait does the same. Resuming before the due time is refused; once due, the run continues
past the settled wait. Suspension requires workspace checkpoints. Named targets advertise
`request_wait: {"kind":"openengine.request-wait/v1"}`. The full contract is in the
[runtime plan reference](../reference/runtime-plan.md#host-run-wait-verifier).

To wait for a person instead of a time, use [Ask a person](ask-a-person.md).

## Long work over many items

A parallel or mapped group has one checkpoint for the whole outer group, and `suspend` lets that
group finish first (see [Observe and control runs](observe-and-control.md)). A map over thousands
of long-lived items is therefore one unit: a resume reruns the whole group, and no item inside it
can wait.

For work that spans days and many items, keep per-item progress outside the graph and process it
in small batches:

- Keep a state ledger with one row per item, its status and when it is next due. A CSV file is
  enough. A long wait for one item becomes a due time in its row.
- Start a short run on an interval, from a scheduler or from a `loop` that waits between rounds.
  A first step reads the due rows, a map processes a small batch, and each item's step records its
  result in the ledger as soon as it finishes.
- Keep the ledger outside the run's workspace. Restoring a checkpoint rolls workspace files back
  to before the whole map, which would erase results the ledger already recorded.
- For an action that must not repeat, such as sending a message, have a verifier check whether it
  already happened before the step acts, and confirm afterwards. A rerun of the batch then skips
  completed items.

The batch size bounds how much a rerun repeats.
