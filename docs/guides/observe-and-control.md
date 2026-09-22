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
  --template software-change \
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
from a named target.

## Resume event and log streams

`watch` follows status events; `logs` follows safe provider and controller log records. Both accept
an opaque cursor and resume strictly after it:

```console
zeroshot watch RUN_ID --after STATUS_CURSOR
zeroshot logs RUN_ID --after LOG_CURSOR
```

Cursors are opaque, so store the last fully consumed value and pass it back unchanged.
`logs --execution EXECUTION_REF` restricts output to one execution selector from status.

The producer records each log timestamp as a positive JavaScript-safe Unix epoch millisecond, and
durable replay preserves it.

## Attach to an active execution

Status can expose an active execution reference. Attach to its interactive event stream with:

```console
zeroshot attach RUN_ID EXECUTION_REF
```

Closing the attachment stream does not cancel the execution.

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
resolve fresh credentials. Provider sessions and previous token usage are not carried into the new
attempt. Selecting a checkpoint replaces later workspace edits, including untracked and ignored
files. Local workspaces must be idle while they are restored.

Local and Docker targets deduplicate checkpoints in a private Restic repository beside their run
storage and advertise `openengine.workspace-checkpoints/v1`. They retain that repository only for a
failed recovery lineage and delete it after a successful run. Cloud exposes recovery through
authenticated hosted run routes, so failed-run checkpoints remain available for 30 days after the
original capsule is removed. Cloud resolves fresh credentials from the admitted connection
references when it starts the successor.

## Stop only with explicit intent

`force-stop` is the destructive run-lifetime command:

```console
zeroshot force-stop RUN_ID
```

The command asks the controller to stop active work and writes the resulting status as JSON. Read
status first if another operator may already have stopped the run, or if it may have completed.

Python follows the same rule. A timeout or task cancellation detaches observation;
`await run.force_stop()` changes the run lifetime.
