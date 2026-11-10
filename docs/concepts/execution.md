---
description: 'Your graph decides what runs next. Learn how zeroshot checks results, routes review feedback and enforces retry limits.'
---

# How execution works

Your graph decides what runs next. After each step, zeroshot validates the result, updates the
shared state and follows the graph's rules. Agents cannot add steps or increase retry limits.

For example, a graph can send a worker's change to two reviewers, return rejected work for repair
and stop after four attempts. The reviewers judge the change; zeroshot enforces the route and limit.
[Build a review loop](../guides/review-loop.md) implements that example.

## Before a run starts { #the-run-boundary }

A run needs four inputs:

| Input         | Purpose                                                       |
| ------------- | ------------------------------------------------------------- |
| Title         | Identifies the run in lists and status output                 |
| Graph         | Defines steps, shared state, routes, limits and results       |
| Initial input | Supplies the task in the format the graph expects             |
| Runtime plan  | Chooses each agent's harness, provider, model and connections |

Before starting a local controller or submitting to a target, zeroshot checks the graph, runtime
bindings and input. For a target-stored profile, it fetches the profile first and applies the same
checks. `--validate-only` runs these checks without starting a run.

## Graph nodes

`step` and `verifier` nodes do the work. Groups control their execution:

| Group                | Behavior                                           |
| -------------------- | -------------------------------------------------- |
| `seq`                | Runs children in order                             |
| `choice`             | Selects a branch from the declared signals         |
| `par`                | Runs branches concurrently and joins their results |
| `loop`               | Repeats a body within a fixed limit                |
| `map`                | Runs a bounded body for array items                |
| `succeed` and `fail` | End the graph                                      |

Routes and conditions are structured data, not executable code or prompt instructions.
The [graph contract](../reference/cluster/graph.md) defines their fields.

## How results choose the next step { #state-reduction-chooses-the-next-step }

Each agent returns a value through its declared output channel. zeroshot validates that value,
updates state using the graph's bindings, then evaluates what runs next.

In a repair loop, a reviewer returns a decision and feedback. The graph routes an acceptance
onward or a rejection back for repair. `maxIterations` still applies if an agent asks to continue.

## Working in parallel { #concurrent-workspace-changes }

Parallel agents share a workspace. Coordinate writers so they do not overwrite each other's
changes, and place reviewers after the writers when they need the finished result. Reviewers see
the same dependencies, generated files and services as workers.

Put Git delivery after all writing and review branches have joined. Graph validation rejects
concurrent delivery alongside another executable node, or inside a map that can have multiple items.

## Run history and cleanup { #durable-run-state }

Each run has an ID and a SQLite ledger. Closing the terminal, disconnecting a client or cancelling
a Python `await` does not stop the controller. Reconnect to inspect the run or stop it explicitly;
see [Observe and control runs](../guides/observe-and-control.md).

The controller cleans up execution activity when the run ends. Cleanup guarantees differ by OS;
[process cleanup](../reference/execution.md#process-cleanup) describes detached child processes.

## Built-in templates are ordinary graphs

Use a built-in graph as it is, or export it to make your own:

```console
zeroshot template list
zeroshot template show software-change
```

`template show` prints a tree, marking executable nodes with `*`. Add `--json` or redirect stdout
to export the complete `GraphSpec`.

| Template          | Purpose                                                   |
| ----------------- | --------------------------------------------------------- |
| `software-change` | Implement, review, repair and deliver a change            |
| `single-worker`   | Run one agent without a review loop                       |
| `pr-review`       | Review a PR and post a concise comment                    |
| `code-review`     | Review a PR in depth and post verified findings           |
| `issue-review`    | Review an issue proposal                                  |
| `auto-research`   | Run experiments and retain results accepted by a reviewer |

`software-change` opens a PR by default. Choose delivery with:

| Option            | Result                                                                      |
| ----------------- | --------------------------------------------------------------------------- |
| `--delivery none` | Keep the accepted change local                                              |
| `--push`          | Push the run branch without opening a PR                                    |
| `--pr`            | Open or update a PR, address feedback and wait for CI; leave merging to you |
| `--ship`          | Follow the same review loop, then merge under the repository's policy       |

Required approval blocks `--ship`, but not `--pr`. For local PR and merge runs, the target branch
is the current branch's upstream, or its local name when no upstream is set. See
[source and delivery setup](targets.md#source-and-delivery-credentials).

PR feedback is enabled by default and restricted to repository writers. `--no-pr-feedback`
disables it; [feedback rules](../reference/execution.md#delivery-feedback) cover permission checks.

`auto-research` runs ten rounds by default; `options.iterations` changes the count. Results stay in
`.zeroshot/research`; `--push` publishes each accepted round. See
[research rounds and repair](../reference/execution.md#auto-research) for the full behavior.
