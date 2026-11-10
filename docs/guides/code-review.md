---
description: 'Review a pull request in depth with the code-review template: parallel specialist reviewers, verified findings, and one Markdown comment.'
---

# Review a PR in depth

The `code-review` template reviews a pull request with parallel specialist reviewers, verifies
their findings, and posts them as one Markdown comment. It takes the same input as
[`pr-review`](review.md) and publishes through the same comment delivery. `pr-review` reproduces
the changed behavior through the full local stack and posts a short verdict. `code-review` reads
the change and reports every verified finding with its location, severity and suggested fix.
It does not install dependencies or start applications, services or browsers.

## What the graph does

```text
triage                 summary and at most six review units
discover (parallel)
  per unit:            two broad reviews, a depth review, a design review
  whole change:        logic, state, contracts, error paths, claims
finalize (parallel)
  per unit:            merge, try to disprove and rewrite the unit's findings
  remaining files:     the same for findings outside every unit
report                 final findings → one Markdown comment
deliver                native comment publication
```

Triage summarizes the change and splits its files into review units of a size one reviewer can
examine carefully, keeping files that must be read together in the same unit. Four reviewers then
examine every unit: two independent broad reviews, a depth review that builds a concrete failing
scenario for each defect, and a design review that compares the code with the rest of the
codebase. At the same time, five reviewers each hunt one kind of failure across the whole change.
The claims reviewer also checks every acceptance criterion in the linked issues, unless the PR or
the caller declares a partial implementation.

For every unit, a finalizer merges duplicate findings for the unit's files, tries to disprove each
one against the real callers, guards and configuration, and rewrites the survivors with exact line
ranges. Another finalizer does the same for findings outside every unit. The report agent orders
the final findings by severity and writes the comment. The native publisher posts it after every
agent has settled. The native header records the captured revisions once.

Every agent is read-only: it inspects the checkout and the diff of the admitted head against its
merge base with the captured base revision, without changing tracked files. A change with N units
runs 5N + 8 agent executions. A crashed agent is retried once. Any other agent failure, or a
triage result with more units than the maps allow, stops the run before publication. Finding
problems is a successful review.

## Prepare the runtime and credentials

The supplied [runtime plan](../assets/code-review/runtime.json) splits the agents across Claude
Code and Codex, so that each unit and the whole change are reviewed by both model families. Save it
as `runtime.json` and pass it through `--runtime-config`. It needs a Claude Code and a Codex login.
Model choices remain in the caller's runtime plan rather than the graph, so a plan with other
models, or a uniform runtime through `--uniform-runtime-config`, works as well.

The supplied plan sets `workspaceCheckpoints: false` for local reviews, skipping file snapshots
and the Restic dependency. History, structured outputs, logs and comment receipts remain durable.
A failed run can restart against its retained checkout, but cannot resume from a workspace snapshot.
Set the same field in a uniform runtime file to use this policy with `--uniform-runtime-config`.

Comment delivery uses the `github` connection's `GH_TOKEN`, as for `pr-review`. Publication
credentials belong to the native node and are not declared on the review agents.

## Run the review

Select a clean checkout already at the PR head. zeroshot refuses a mismatch instead of switching
or resetting the worktree.

```console
zeroshot run --title "Code review PR 123" \
  --template code-review \
  --task-from https://github.com/OWNER/REPOSITORY/pull/123 \
  --runtime-config runtime.json \
  --cwd /path/to/pr-checkout \
  --submission-key code-review-pr-123
```

`--validate-only` prepares and validates without executing agents. For hosted execution, set
`workspaceCheckpoints` to `true` (or remove the field) before adding `--target platform`; named
targets currently reject checkpoint opt-out. Source comes from URL resolution, so a client checkout
is unnecessary.

## Input and result

The input and result match [`pr-review`](review.md#input-and-result). `--task-from` captures
`{url, context}` with the PR title and body, the complete `context.reviewScope`, and the linked
issues. Optional caller `instructions` can narrow the review or declare a partial implementation.
Like `pr-review`, the template requires comment delivery. The result retains the comment
`receipt`, and each agent's findings stay in run history.

With prepared JSON passed through `--input`, the local checkout's upstream must name the captured
PR base branch, for example `origin/main`. `--task-from` supplies that source identity itself.
