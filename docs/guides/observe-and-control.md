# Observe and control runs

Run state persists when an observation client leaves, and reconnecting does not change the run
lifetime.

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

## Stop only with explicit intent

`force-stop` is the destructive run-lifetime command:

```console
zeroshot force-stop RUN_ID
```

The command asks the controller to stop active work and writes the resulting status as JSON. Read
status first if another operator may already have stopped the run, or if it may have completed.

Python follows the same rule. A timeout or task cancellation detaches observation;
`await run.force_stop()` changes the run lifetime.
