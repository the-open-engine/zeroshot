---
description: 'Process cleanup, research rounds and the permissions required for delivery feedback.'
---

# Execution and delivery details

[How execution works](../concepts/execution.md) explains graph execution.

## Process cleanup

When a run closes, zeroshot atomically closes its execution activity; a late process cannot appear
after the terminal result.

On Linux, each local command has a private supervisor that also owns descendants which detach with
`setsid`, change process group, or double-fork. It stops and reaps them when the command finishes,
the run cancels, or the controller dies. Workspace ownership remains held through cleanup, so a
successor cannot resume while that command's descendants can still write. This uses the kernel's
subreaper support and procfs; it needs no root access or cgroup setup. Windows uses Job Objects.
macOS retains process-group cleanup; descendants that detach can outlive the command there.

## Auto-research

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

## Delivery feedback

PR and merge delivery consider feedback by default. Use `--no-pr-feedback` to ignore PR discussion;
the [`git_delivery` binding](runtime-plan.md#git_delivery) describes the runtime
setting.

Automatic repairs accept feedback only from current writers on the target repository. GitHub
confirms effective write or admin permission for issue comments, submitted reviews (including
unwritten change requests), inline comments and replies. GitLab confirms active Developer-or-higher
project membership, including inherited and invited membership, for discussion notes. The account
must be active. Enterprise Edition also requires active `membership_state`; awaiting membership is
ignored. Community Edition omits that field, so its response is accepted only after instance metadata
confirms the edition. Unknown or malformed membership status fails the feedback read. Permissions
are checked again on each inspection and bound to the author's stable numeric identity. Organization
association, PR authorship, requested-reviewer status and bot status grant no exception. Confirmed
read-only access or nonmembership is ignored and does not start another worker/reviewer round.
Failed or incomplete permission lookups stop delivery or use its existing retry handling.
