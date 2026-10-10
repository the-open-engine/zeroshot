# Build a review loop

This guide builds a custom graph that sends one coding task through a worker and two independent
reviewers. When either reviewer rejects the change, the loop runs the same worker again with both
reviewers' feedback. The run succeeds when both accept, and fails after four passes.

The built-in `software-change` template already does this with more recovery paths. Build the loop
yourself when you want to change the reviewers, their instructions, or the bounds.

## Before you start

This guide assumes you have completed [Run a first task](../getting-started/first-run.md): Zeroshot
is installed, the Codex CLI is signed in, and you have a Git repository with an HTTP service to
change.

!!! warning "The worker edits the current worktree"

    Commit or stash work you want to protect, and run the commands from the repository you intend to
    change.

## Files

The example has three files, plus an optional runtime plan for
[cross-vendor review](#cross-vendor-review). Download them or copy them from the sections below, and
keep them outside the repository you are changing, for example in `~/review-loop/`, so they stay out
of the change.

| File                                                                                                   | Purpose                                       |
| ------------------------------------------------------------------------------------------------------ | --------------------------------------------- |
| [`input.json`](../assets/review-loop/input.json)                                                       | The task                                      |
| [`review-loop.graph.json`](../assets/review-loop/review-loop.graph.json)                               | Worker, reviewers, loop, and terminal result  |
| [`review-loop.runtime.json`](../assets/review-loop/review-loop.runtime.json)                           | Harness, provider, models, and session scopes |
| [`review-loop.cross-vendor.runtime.json`](../assets/review-loop/review-loop.cross-vendor.runtime.json) | Optional: Codex worker, Claude Code reviewers |

The graph controls order and data flow. The runtime plan binds only the three nodes that run agents.

The excerpts in steps 1 to 4 leave out fields; the complete graph is in step 5.

## 1. Define input and state

A graph starts with a profile, an initial input type, and a policy. The caller supplies only the
task:

```json
"initialInput": {
  "kind": "record",
  "fields": {
    "task": { "type": { "kind": "string" }, "required": true }
  }
}
```

The root `seq` declares a state record with `task` plus one feedback field per reviewer,
`acceptanceFeedback` and `codeFeedback`. The root may add required fields that have an implicit
empty value, so both feedback strings start as `""` without appearing in `input.json`.

Every structural node (`seq`, `loop`, `par`, `choice`) declares its own state type, so the graph
repeats this record. Input bindings copy state into an agent's input. Promoted state paths copy
updated state from a group back to the group that encloses it.

## 2. Add the worker

The worker is a `step`. It reads the task and both feedback fields:

```json
{
  "kind": "step",
  "name": "worker",
  "worker": "builtin.agent.software-worker@1",
  "instructions": "Implement the requested change. If reviewer feedback is present, address both diagnostics before returning. Work in the shared repository and run focused checks.",
  "output": { "kind": "null" },
  "writeBindings": [],
  "attempts": 1
}
```

The full node also declares an `input` record and three `inputBindings`, one per field. Its output
is `null` because the worker changes files in the shared workspace rather than returning data.

The `worker` value is a label local to this graph. The `builtin.` names only mirror the built-in
template; they add no behavior. Admission derives each worker's contract from the node's declared
input, output, signals, and diagnostic, so a label used on several nodes must keep the same
declaration and binding kind. Every agent node needs authored `instructions`.

## 3. Run two reviewers in parallel

A `par` group named `reviews` runs two verifiers against the same workspace. Each verifier returns
a `verdict` signal and a diagnostic, and writes the diagnostic message into its feedback field:

```json
{
  "kind": "verifier",
  "name": "acceptance",
  "worker": "builtin.agent.acceptance-verifier@1",
  "signals": { "verdict": ["accepted", "rejected"] },
  "writeBindings": [
    {
      "value": { "node": "acceptance", "channel": "diagnostic", "path": ["message"] },
      "target": ["acceptanceFeedback"]
    }
  ]
}
```

The `code` verifier has the same shape and writes `codeFeedback`. Zeroshot tells verifiers not to
edit the material under review; that is an instruction, not a filesystem boundary.

The `all` join waits for both reviewers. `reviews` promotes both feedback paths into the iteration
state, and the iteration promotes them into the loop state, so the next worker pass can read them.

## 4. Stop on two acceptances

The loop body is a `seq` of the worker and `reviews`. Loops are do-while: `until` is checked after
each pass, so the worker always runs at least once.

```json
"until": {
  "kind": "all",
  "guards": [
    {
      "kind": "in",
      "value": { "name": "acceptance", "source": "signal", "field": "verdict" },
      "labels": ["accepted"]
    },
    {
      "kind": "in",
      "value": { "name": "code", "source": "signal", "field": "verdict" },
      "labels": ["accepted"]
    }
  ]
},
"maxIterations": 4
```

A loop reports `converged` or `exhausted` through its `terminated` group control. A `choice` after
the loop turns that control into an explicit terminal:

```json
{
  "kind": "choice",
  "name": "loop_result",
  "branches": [
    {
      "when": {
        "kind": "in",
        "value": { "name": "review_loop", "source": "group", "field": "terminated" },
        "labels": ["converged"]
      },
      "node": { "kind": "succeed", "name": "done", "output": { "kind": "null" }, "bindings": [] }
    }
  ],
  "otherwise": {
    "kind": "fail",
    "name": "review_attempts_exhausted",
    "reason": "review_attempts_exhausted"
  }
}
```

## 5. The complete graph

??? example "review-loop.graph.json"

    ```json
    --8<-- "assets/review-loop/review-loop.graph.json"
    ```

The structural nodes `run`, `review_loop`, `review_iteration`, `reviews`, and `loop_result` do not
run agents, so the runtime plan has no entries for them.

## 6. Bind the agents

The runtime plan has one binding per executable node, keyed by node name: `worker`, `acceptance`,
and `code`.

```json title="review-loop.runtime.json"
--8<-- "assets/review-loop/review-loop.runtime.json"
```

The worker uses `node_instance`, so each loop pass continues the same provider conversation. The
reviewers use `execution`, so each review starts a fresh session and does not see the previous pass.
The graph state still carries the feedback explicitly either way.

Replace `YOUR_MODEL_ID` with a model the provider accepts. Nodes can use different models, effort
values, or [harnesses and providers](#cross-vendor-review); size applies to the whole run. The
bindings omit `connections`, so a local Codex run reuses the Codex CLI's login, and a Docker or
cloud target derives the canonical `OPENAI_API_KEY` requirement. See
the [RuntimePlan reference](../reference/runtime-plan.md) for every field.

## 7. Validate, then run

The input supplies only the task:

```json title="input.json"
--8<-- "assets/review-loop/input.json"
```

From the repository you want to change, check all three files without starting a run. Validation covers JSON shape, graph data flow, loop
termination, runtime coverage, and the initial input:

```console
zeroshot run \
  --title "Review loop validation" \
  --graph ~/review-loop/review-loop.graph.json \
  --runtime-config ~/review-loop/review-loop.runtime.json \
  --input ~/review-loop/input.json \
  --validate-only
```

Successful validation writes `{"valid":true}`.

Remove `--validate-only` to start the run:

```console
zeroshot run \
  --title "Health-check endpoint" \
  --graph ~/review-loop/review-loop.graph.json \
  --runtime-config ~/review-loop/review-loop.runtime.json \
  --input ~/review-loop/input.json
```

[Observe and control runs](observe-and-control.md) covers status, logs, and stopping.

The same files run on a target with `--target cloud` (see
[Connect Zeroshot Cloud](../getting-started/install.md#connect-zeroshot-cloud)) or the name of a
[direct target](../concepts/targets.md#direct-target). A target checks out the worktree's pushed
upstream branch, not local uncommitted changes, and resolves `OPENAI_API_KEY` from the submission
environment or its connection store. The graph has no delivery node, so a target run does not push
the accepted change. Add a Git delivery verifier after the loop, or use the `software-change` template with
`--push`, `--pr`, or `--ship`, when the result must leave the target.

## Cross-vendor review

A reviewer from the worker's model family can share its blind spots. An agent binding can carry its
own [lane](../reference/runtime-plan.md#per-node-lanes), a harness and provider pair that overrides
the plan's default. This runtime plan keeps the worker on Codex and runs both reviewers on Claude
Code; the graph does not change:

```json title="review-loop.cross-vendor.runtime.json"
--8<-- "assets/review-loop/review-loop.cross-vendor.runtime.json"
```

Replace `YOUR_OPENAI_MODEL_ID` with a model OpenAI accepts and `YOUR_ANTHROPIC_MODEL_ID` with one
Anthropic accepts. Pass the file to `--runtime-config` in [step 7](#7-validate-then-run) in place of
`review-loop.runtime.json`.

A local run needs both the Codex CLI and Claude Code installed and signed in. Before the run starts,
Zeroshot checks that `codex` and `claude` are on `PATH`; it does not check login state, so a missing
Claude Code login fails at the first review. On a target, the run requires both `OPENAI_API_KEY` and
`ANTHROPIC_API_KEY`, and the target must advertise `openengine.node-runtime-lanes/v1`. The CLI
refuses to send the plan to a target that does not.
