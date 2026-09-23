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

## Concurrent workspace changes

`par` branches and `map` items can run multiple writing agents at once. Writers share the run's
workspace, so the graph and agent instructions must coordinate edits to the same files. Hosted
verifiers work in disposable copies; copying while a writer is active does not provide an atomic
snapshot. Place verification after the writers when it needs their completed changes.

Sequence Git delivery after the writing branches or map have joined. Graph validation rejects
parallel delivery and writing, including delivery in a map that can have multiple items. Delivery
can run alongside verifiers, which are instructed not to edit the candidate. A delivery receipt can
certify run success only when every other writer settled before that delivery execution started.

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

`auto-research` runs exactly ten iterations. Mapped explorer, synthesizer, and challenger scouts
propose distinct ways to advance the caller's charter. A one-time read-only preflight requires the exact
scout, judge, and experiment sets before the loop; mapped evidence, method, and progress judges
then review it independently. Ignored scratch files carry their handoffs. Unanimous `adopt` keeps
working changes. A supported negative or inconclusive result becomes
`record_only` and restores those changes; invalid, incomplete, or unsafe evidence becomes `abort` and
also restores them. Finalized records live under
`.zeroshot/research/iterations/`; the charter, state, summary, and backlog live at the research root.
Scratch handoffs and backups use per-iteration directories. A scout, selector, experimenter, or judge
execution failure becomes a finalized aborted iteration; restoration runs when an experiment may have
changed the workspace, and the bounded loop continues. A recorder or recovery failure stops before
the next iteration. The charter distinguishes required invariants from
optional progress measures. If evidence proves that the retained workspace violates an invariant,
the selector prioritizes repair and judges treat a verified repair as adopt even when it does not
clear an optional optimization threshold. Mutable state and summaries report the retained workspace
and its invariant status separately from the best supported historical findings, so a result from a
restored artifact is never presented as current.
The template supports local results and `--push`. With `--push`, a read-only worker derives commit
metadata from the finalized ledger, then the graph revisits its single Git delivery node after each
iteration rather than only at graph completion. Manifest failures, delivery repair requests, and
failed or rejected receipt audits stop the run instead of publishing an unreviewed repair.

PR and merge delivery consider feedback by default. Use `--no-pr-feedback`, or set
`pullRequestFeedback` to `ignore` on the delivery runtime binding, when the run should ignore PR
discussion. CI, conflict, freshness, and merge-policy checks remain active.
