# Cluster Protocol v1 operational lifecycle

This contract adds operational `update` and `stop` controls to an admitted run. The Rust protocol
types are authoritative. The deterministic testkit backend proves the state machine and durable
records; it is not a native graph scheduler, worker executor, or process-freezing runtime.

## Update

`update({labels?, logLevel?, suspended?, ifGeneration, idempotencyKey})` requires at least one
operational field. `labels` is a complete replacement map with at most 64 bounded key/value pairs;
`logLevel` is one of `trace`, `debug`, `info`, `warn`, or `error`; `suspended` is boolean. The closed
request rejects graph, input, policy, worker, null, and unknown fields. Update preserves the graph,
compiled identity, root input, generation, run ID, and admission cursor.

Suspension is a durable dispatch gate, not a process freeze. It denies every new successor permit
but leaves existing leases alive. An existing black-box call may finish while suspended and append
its verified output. Resume returns the gate to `active`, after which successor dispatch continues
from the latest durable cursor.

## Stop

`stop({mode, ifGeneration, idempotencyKey})` accepts `drain` or `force`.

- Drain closes the dispatch gate and waits for every existing lease. The final verified completion
  atomically appends the single terminal `finished` record. With no in-flight lease, stop finishes
  immediately. Drain invokes no `onComplete` or other hook absent from the authored graph contract.
- Force closes the gate, signals every lease cancellation token, records each cancelled turn as
  structural void state with no outcome, and appends the single terminal `finished` record. A force
  request escalates an existing drain; a drain request never downgrades force.

The terminal `finished` record is the last lifecycle event. Late dispatch, completion, update, and
new stop mutations are rejected without appending lifecycle or verified-I/O records. Exact replay
of a previously accepted idempotency key remains available and cannot append a second terminal
record.

## CAS, idempotency, and acknowledgements

Both methods require an exact generation CAS. Fingerprints bind the method and canonical validated
parameters except `idempotencyKey`. Same-key replay returns the original receipt with
`deduped:true`; changed parameters or cross-method key reuse returns `IDEMPOTENCY_REUSE`.

Stop receipts acknowledge the accepted mode, effective monotonic mode, and durable lifecycle
state. They do not claim that external side effects were rolled back, that cancellation made an
already-started call side-effect-free, or that a worker never observed the request. Force prevents
late output from becoming verified protocol output; it cannot undo effects outside this protocol.

## Status and fixture boundary

Admitted status includes labels, log level, dispatch state, optional stop mode, and in-flight count.
The resource phase remains `running` during active, suspended, and draining operation and becomes
`finished` exactly once. `InMemoryAdmissionStore` serializes admission and lifecycle mutations under
one mutex solely for deterministic conformance tests. Scripted `running` and `finished` states do
not imply native node execution or production cancellation.
