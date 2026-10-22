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
[graph contract](../reference/cluster/graph.md), and [Build a review loop](../guides/review-loop.md)
builds a custom graph step by step.

## Concurrent workspace changes

`par` branches and `map` items can run multiple writing agents at once. Writers share the run's
workspace, including reviewers, so the graph and agent instructions must coordinate edits to the
same files. Reviewers see the same installed dependencies, generated output, and services as workers.
Place verification after the writers when it needs their completed changes.

Sequence Git delivery after the writing branches or map have joined. Graph validation rejects
delivery alongside any other executable node, including reviewers, and delivery in a map that can
have multiple items. A delivery receipt can certify run success only when every other executable
settled before that delivery execution started.

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

The executable contains `single-worker`, `software-change`, and `auto-research`. Each produces the
public `GraphSpec` that custom submissions use:

```console
zeroshot template list
zeroshot template show software-change
```

`software-change` has four delivery choices. The default keeps the accepted change local. `--push`
publishes the managed run branch without opening a pull request. `--pr` opens or updates a pull
request, processes visible review feedback, waits for required CI and a conflict-free head, then
stops without merging. `--ship` uses the same feedback loop before it follows the repository's merge
policy and confirms the merged revision. Missing approval doesn't block `--pr`; it still blocks
`--ship` when GitHub requires it.

For local `--pr` and `--ship` runs, the target branch is the current branch's upstream when one is
configured; otherwise, Zeroshot uses the local branch name. Preparation verifies that the selected
remote contains this target before agent execution. To target `main` from a fresh worktree,
run `git branch --set-upstream-to=origin/main`, or create the worktree from `origin/main` so Git
sets the tracking branch automatically. If the remote cannot be reached or authenticated,
preparation reports that the target branch could not be verified.

`auto-research` runs ten iterations by default. Optional run input `options.iterations` selects a
positive count bound at admission, subject to the engine's general execution and loop budgets.
Each normal round has five model executions:

1. Three independent read-only scouts propose exploration, synthesis, and challenge directions.
2. One researcher selects the parent and experiment, verifies/restores candidate bytes, runs the
   experiment, and prepares a supported retained result with compact evidence.
3. A fresh read-only reviewer accepts the proposed disposition, requests concrete corrections, or
   approves a justified early stop. A supported non-adoption can be accepted without replacing the
   incumbent. Plateau alone does not justify stopping before the requested count.

The scouts and researcher keep separate continuing sessions by default; the reviewer starts fresh.
Required corrections return to the same researcher without repeating scouting or consuming another
research round, then receive fresh review. This work/review cycle has a 64-pass safety limit.
Routine file, tool, or archive errors can be repaired by the researcher. Missing optional historical
archives and cosmetic bookkeeping issues are advisory when the current result and retained state
are supported. Native reviewer outcomes establish approval; no separate file audit counter is
required. There are no bootstrap, staging, finalization, or extra audit model passes.

Records live under `.zeroshot/research` at the shared workspace root, including for tasks in
subdirectories. The researcher keeps the charter, current summary, backlog, round evidence, and
restorable candidate source there. After two accepted non-adoptions, or diminishing returns across
`max(1, ceil(iterationLimit / 10))` accepted rounds on one approach, it explicitly reconsiders a new
mechanism, an archived parent, and a discriminating measurement. These are reasoned alternatives,
not a mandatory parent switch or a fixed gain threshold.

The template supports local results and `--push`. With `--push`, the reviewer supplies title and
description and one trusted Git delivery node publishes each accepted round. Research agents never
use Git. Exhausted review, unrecovered execution errors, delivery errors, and delivery repair
requests stop the run explicitly; unreviewed work is never published. Existing saved profiles and
admitted runs keep their stored graph and runtime.

PR and merge delivery consider feedback by default. Use `--no-pr-feedback` to ignore PR discussion;
the [`git_delivery` binding](../reference/runtime-plan.md#git_delivery) describes the runtime
setting.
