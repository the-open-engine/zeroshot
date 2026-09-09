# How execution works

Zeroshot keeps control flow separate from model behavior through a caller-authored graph. After
admission, the engine runs an executable node and checks its typed output; state reduction records
that output before choosing the next edge.

## The run boundary

A submission contains four pieces of caller-owned data:

| Input         | Purpose                                                                    |
| ------------- | -------------------------------------------------------------------------- |
| Title         | Human-readable identity for lists and status output                        |
| Graph         | Nodes, state, transitions, bounds, and terminal results                    |
| Initial input | The unchanged value checked against the graph's declared input type        |
| Runtime plan  | Harness, provider, model, session scope, and named connection requirements |

For inline graphs and built-in templates, Zeroshot completes admission before it starts a local
controller or contacts a target. The CLI first fetches a target-stored profile, then sends its graph
and runtime through the same checks. Those checks cover schema shape, graph semantics, bounds,
runtime bindings, and the initial input; `--validate-only` runs them without creating a run.

## Graph nodes

Executable nodes are `step` and `verifier`. Groups compose them:

| Group                | Behavior                                                      |
| -------------------- | ------------------------------------------------------------- |
| `seq`                | Evaluates children in order                                   |
| `choice`             | Selects an authored branch from typed control signals         |
| `par`                | Runs branches concurrently and joins them by an authored rule |
| `loop`               | Provides a bounded do-while structure                         |
| `map`                | Applies a bounded body to array items                         |
| `succeed` and `fail` | End the graph                                                 |

Selectors, guards, and bindings are structured data. They are not snippets of JavaScript, JSONPath,
shell code, or prompt text. Full field rules live in the
[graph contract](../reference/cluster/graph.md).

## State reduction chooses the next step

An agent returns a value through a declared output channel. Zeroshot checks that value, applies
authored bindings, and records the new state before it evaluates another node. Model output cannot
add an edge or raise a bound.

Inside a repair loop, a reviewer emits a finite signal with a typed diagnostic. The graph routes that
signal to success or another iteration, and `maxIterations` still applies if an agent asks to
continue.

## Durable run state

Each run has a public ID backed by a SQLite ledger. Status and safe-log events carry opaque cursors
for strict resume, so detaching a client, losing a terminal, or cancelling a Python `await` does not
stop the controller.

When a run closes, Zeroshot atomically closes its execution activity; a late process cannot appear
after the terminal result. [Observe and control runs](../guides/observe-and-control.md) covers the
read and stop commands.

## Built-in templates are ordinary graphs

The executable contains `single-worker` and `software-change`, and both produce the public
`GraphSpec` that custom submissions use:

```console
zeroshot template list
zeroshot template show software-change
```

`software-change` adds pull-request or merge delivery through `--pr` or `--ship`. That delivery is a
graph node with its own runtime binding rather than an unrecorded agent side effect.
