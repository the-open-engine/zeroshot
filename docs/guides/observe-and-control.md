---
description: 'Observe durable run state in the browser or CLI, inspect node transcripts, reconnect to a run and stop work without changing its history.'
---

# Observe and control runs

Run state persists when an observation client leaves, and reconnecting does not change the run
lifetime.

## Browser

Run [`zeroshot ui`](../getting-started/install.md#open-the-workspace-ui), then select a run in **Runs**.
The read-only graph uses that run's admitted definition. Select a node for its inputs, outputs,
decisions, and transcript; use the timeline to replay its history. Active runs update live.
Seeking pauses following; **Go live** returns to the latest event. Scrolling a transcript keeps
the graph live and follows new output only when you reach the bottom.

For a direct target, open `/ui/` on its configured public origin. Closing either browser view
leaves the run active.

## Foreground and detached submission

`zeroshot run` follows the durable event stream by default and writes NDJSON. Add `--detach` to
return after submission:

```console
zeroshot run \
  --title "Update parser" \
  --template software-change --delivery none \
  --input input.json \
  --uniform-runtime-config runtime.json \
  --detach
```

Ctrl-C closes the terminal stream and leaves the controller running.

## Inventory and current status

These commands are read-only and return JSON:

```console
zeroshot list
zeroshot status RUN_ID
```

A run ID belongs to the controller or target that created it. Pass `--target NAME` when reading one
from a named target. Finished status separates graph-authored `terminalResult.output` from optional
runtime-confirmed `metadata.deliveryReceipt`; graphs without delivery omit the receipt.

## Token usage

`metadata.tokenUsage` contains run totals summed across agent invocations, including retries,
correction turns and auxiliary agents such as postmortem specialists. Counters are summed as each
harness reports them; input-token accounting can differ between harnesses.

| Field                      | Meaning                                               |
| -------------------------- | ----------------------------------------------------- |
| `inputTokens`              | Input tokens reported by the harness.                 |
| `outputTokens`             | Output tokens reported by the harness.                |
| `cacheReadInputTokens`     | Input tokens reported as read from a prompt cache.    |
| `cacheCreationInputTokens` | Input tokens reported as written to a prompt cache.   |
| `complete`                 | Whether every invocation reported usable token usage. |

When `complete` is `false`, the totals retain known usage but at least one invocation's usage is
missing or unusable. An omitted cache counter means at least one invocation did not report that
counter; its run total is unknown, not zero. Cache counters can be omitted even when `complete` is
`true`. Script steps record zero for all four counters and preserve known cache totals.

## Failure evidence and mapped executions

Failed `terminalResult.context` separates the known runtime cause (`primary`) from recent recorded
execution errors (`recorded`) and verifier results (`verifiers`). Each verifier result is the latest
recorded result for that graph node and map path. Its bounded, credential-redacted `message` contains
the graph's actual `signals` and `diagnostic`; the runtime assigns no general meaning to their names
or labels. A later result replaces an earlier result in this summary, while logs retain both.
Recorded evidence can include recovered attempts or parallel work and does not prove that a result
caused the terminal failure. An independently known runtime cause remains in `primary`.

`exhaustedLoops` identifies completed exhausted loop visits with `node`, `completedIterations`, and
an optional `mapIndices` path. The count is the actual number of completed loop rounds, including
all work within each round; execution retries and verifier completion counts do not add rounds.
Completed exhausted loops in settled branches of an unreachable parallel join are retained.
Discarded race and map-item paths do not contribute loop entries or consume the summary bound.
A converged loop does not appear as exhausted. The summary keeps at most five entries in each
collection. `incomplete: true` means evidence was omitted or could not be retained. This warning
persists even if later loop visits converge and remove all retained loop entries. Full details may
still be available in logs. Failure context freezes with the main outcome, before auxiliary work.

Each diagnostic can carry an `execution` object with the node name and stable opaque execution
selector, and a `cursor` pointing to its retained log. Missing cursors cannot be used for log
navigation. Status/watch `activeExecutions` and execution-associated log notifications also expose
`mapIndices`: zero-based indices, outermost map first. For example, `[1]` is the second item and
`[1, 2]` is its third nested item. Outside maps the field is omitted (older data also reads as an
empty path). Indices supplement execution identity without changing its opaque selector.
Successful terminal results keep their existing representation.

## Resume event and log streams

`watch` follows status events; `logs` follows safe provider and controller log records. Both accept
an opaque cursor and resume strictly after it:

```console
zeroshot watch RUN_ID --after STATUS_CURSOR
zeroshot logs RUN_ID --after LOG_CURSOR
```

Cursors are opaque, so store the last fully consumed value and pass it back unchanged.
`logs --execution EXECUTION_REF` restricts output to one execution selector from status.

`watch` exits 0 when the run succeeded. When the run finished failed, it prints the same failure
summary on stderr as a failed foreground `run` and exits 1, so a script can start a run with
`--detach` and wait for its result with `zeroshot watch RUN_ID`. `logs` exits 0 when its stream
ends, whatever the run's result. The [CLI reference](../zeroshot-cli.md) lists the exit status of
`run`, `watch` and `logs`.

The producer records each log timestamp as a positive JavaScript-safe Unix epoch millisecond, and
durable replay preserves it.

## Attach to an active execution

Status can expose an active execution reference. Attach to its interactive event stream with:

```console
zeroshot attach RUN_ID EXECUTION_REF
```

Closing the attachment stream does not cancel the execution.

## Download a retained workspace

Download the latest retained files without starting another run:

```console
zeroshot export-workspace RUN_ID --target platform --output workspace.tar
zeroshot export-workspace RUN_ID --target platform --from-checkpoint CHECKPOINT_ID --output checkpoint.tar
```

Use `zeroshot checkpoints RUN_ID --target platform` to find an entry checkpoint ID. Omit
`--from-checkpoint` to export the latest retained workspace. The target authorizes access and
checks whether the retained state is still available. Other named hosted targets work the same way,
using their existing device login or `ZEROSHOT_API_KEY` and organization selection.

The target must advertise [workspace export](../concepts/targets.md#workspace-export-protocol).
An older target without this capability returns a clear unsupported error. Local and direct-target
exports are unavailable.

`--output` is required and must name a UTF-8 path that does not exist, including directories and dangling
symlinks. The CLI streams the raw tar into a private temporary file beside that path and publishes it
atomically without overwriting a competing file. It waits for the entire HTTP transfer, including
its framing, even after a complete tar has arrived. Transfer failures and Ctrl-C remove temporary
output and fail the command. The archive is never extracted and its contents are never executed.

## Restart or resume a failed run

Failed local runs and direct targets retain their workspace. Restart the original graph using its
latest files with:

```console
zeroshot resume RUN_ID
```

To resume at an earlier node, list its available input checkpoints and select one:

```console
zeroshot checkpoints RUN_ID
zeroshot resume RUN_ID --from-checkpoint CHECKPOINT_ID
```

Add `--target NAME` to both commands for a named target. Checkpoint listings are paginated; use
`--after CHECKPOINT_ID` with the returned `nextAfter` value to fetch the next page.

A selected checkpoint restores the files from immediately before its node and the completed
prerequisite outputs. The selected node runs again. Parallel and mapped groups have one checkpoint
for the whole outer group, including nested groups and all mapped batches; individual concurrent
writers do not have separate snapshots. Read-only boundaries can share the same workspace bytes.

Both modes create a new run ID, keep the original graph, input, source, and delivery lineage, and
resolve credentials again. The successor does not inherit provider sessions, secrets, or previous
token usage. Selecting a checkpoint replaces later workspace edits, including untracked and ignored
files. Restart restores the latest completed snapshot and starts the graph at its root. A sudden
target loss can precede the next snapshot boundary, so the last writer may run again. Local workspaces
must be idle while they are restored.

A local resume restores the snapshot before any agent starts. That preparation may take up to 10
minutes. For larger workspaces or slower disks, set `ZEROSHOT_PREPARATION_TIMEOUT` to a whole number
of seconds from 1 to 86400 before `zeroshot resume`. When a restore fails, or its extraction reaches
that deadline, the error names the step that stopped and its elapsed time. Replacing the workspace
files, once started, is allowed to finish. If it is still running 30 seconds after the deadline,
the resume is stopped with an error that names the deadline. Either way the run stays recoverable.

A restore never changes the checkout before the complete snapshot is extracted. If a crash
interrupts it while it replaces files, the checkout can stay partially restored. The next
`zeroshot resume` then removes the interrupted staging copy beside the workspace. It finishes the
restore with the same or another checkpoint before any agent runs. A resume that would start
without restoring a snapshot is refused until a restore completes.

Local and Docker targets deduplicate checkpoints in a private Restic repository beside their run
storage and advertise `openengine.workspace-checkpoints/v1`. They retain that repository only for a
failed recovery lineage and delete it after a successful run. Platform exposes recovery through
authenticated hosted run routes, so failed-run checkpoints remain available for 30 days after the
original capsule is removed. Platform resolves declared connections from their original references.
Direct-target resume reuses the original source connection authorization and resolves fresh values;
export `GH_TOKEN` again and pass `--forward-gh-token` when the run relied on the shell's token
instead of the target's stored `github` connection.

## Stop only with explicit intent

`force-stop` is the destructive run-lifetime command:

```console
zeroshot force-stop RUN_ID
```

The command asks the controller to stop active work and writes the resulting status as JSON. Read
status first if another operator may already have stopped the run, or if it may have completed.

Python follows the same rule. A timeout or task cancellation detaches observation;
`await run.force_stop()` changes the run lifetime.
