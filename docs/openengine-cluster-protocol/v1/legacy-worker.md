# Legacy Zeroshot cluster worker

`createLegacyClusterWorker()` exposes the current Node Zeroshot engine as the bounded
`legacy.zeroshot.ship@1` worker. It is an implementation binding for a parent process, not an Open
Engine Cluster Protocol transport or a graph executor.

```js
const { createLegacyClusterWorker } = require('@the-open-engine/zeroshot/lib/cluster-worker');

const worker = createLegacyClusterWorker({
  profileRegistry,
  artifactResolver,
  artifactReceiptSink,
  engineAdapter,
});

await worker.start({
  source: 'issue',
  issue: 'https://github.com/example/project/issues/123',
  artifacts: [],
  isolationProfile: 'isolation.worktree@1',
  providerProfile: 'provider.default@1',
});

for await (const event of worker.events()) {
  // Live lifecycle transitions only; no durable cursor or replay guarantee.
}

const receipt = await worker.result();
```

The five public operations are `start(request)`, `status()`, `events()`, `stop()`, and `result()`.
One facade owns at most one cluster. There is no guidance, permission, authentication, writable
attach, or arbitrary-input operation.

## Closed input

Requests validate against the generated `LegacyShipRequest` schema. They select exactly one source:

- `issue`: nonempty `issue`, no prompt, and no artifacts;
- `prompt`: nonempty `prompt`, no issue, and no artifacts;
- `artifact`: one or more byte-free `ArtifactRef` receipts and no issue or prompt.

`isolationProfile` and `providerProfile` are opaque registry handles. Unknown fields are rejected,
so payloads cannot carry credentials, tokens, environment maps, endpoints, models, timeouts,
filesystem paths, commands, or launch flags. The production deployment registry resolves only
worktree, Docker, PR, or ship isolation. A missing, unknown, or non-isolated profile fails before
the engine allocates a ledger, worktree, or container.

Artifact input fails before allocation unless the embedding product injects a real resolver. After
the engine allocates the worktree or Docker workspace but before any agent starts, that resolver
materializes declared receipts as read-only content inside the allocated isolation and returns an
engine-private, byte-free manifest. Echoing receipts without making their content readable is not a
resolver. Artifact receipt sinks convert declared durable output to canonical byte-free receipts.
Without a sink, successful output contains an empty artifact list. Raw provider and agent output
remains engine diagnostic data and never enters the stable result.

## Lifecycle and termination

The monotonic lifecycle is `idle`, `starting`, `running`, optional `stopping`, then exactly one of
`completed`, `failed`, `timed_out`, `stopped`, or `malformed`. Completion and failure are folded from
the engine cluster record and durable `CLUSTER_COMPLETE` or `CLUSTER_FAILED` ledger messages. PID
liveness can add diagnostics but cannot manufacture success.

Completion, declared failure, malformed output, the registry-owned execution deadline, and explicit
stop race under one terminal latch. The first accepted authority is immutable; late messages do not
change the receipt or emit another terminal event. Profile resolution, artifact staging, engine
start, and artifact receipt collection all remain within that deadline and can be preempted by
explicit stop. Completion becomes authoritative only after its canonical receipt has been bounded
and validated, so a stalled receipt sink cannot suppress timeout or stop. Profile resolution,
artifact staging, and receipt collection receive one cancellation signal. If an injected operation
ignores that signal and completes late, its registry `release` or port `cleanup` hook retains
resource ownership and runs after the terminal receipt is fixed. A late operation or cleanup
failure is reported through the injected cleanup-failure reporter, or as a process warning when no
reporter is configured; it is never silently discarded or allowed to rewrite terminal truth.

A synchronous pre-allocation rejection leaves the facade idle and releases its one-start claim.
`stop()` and `result()` then reject as not started, and the caller may retry with a valid request.

`stop()` delegates to the engine stop path and waits no longer than the deployment profile's
shutdown deadline. It reports `effective: false` when the engine rejects or does not acknowledge
stop in time. The acknowledgement deadline does not abandon resource ownership: if engine
allocation occurs later while start is still pending, background cleanup stops that cluster. Stop
always reports
`externalEffectsRolledBack: false`: provider, repository, or tool side effects may already have
happened.

Engine completion must contain either a canonical `LegacyShipResult` or an explicit bounded summary.
Engine failures must contain a valid closed error-code/reason pair. Missing completion data, invalid
failure values, and invalid failure pairs terminate as `malformed`; the facade never fills them with
success or crash defaults. Engine status observation is synchronous so a durable-ledger observation
failure can immediately fail closed instead of becoming an unobserved rejected promise.

## Executable

`zeroshot-cluster-worker` reads bounded NDJSON commands from stdin and writes only NDJSON protocol
frames to stdout. Diagnostics use stderr. A process owns one worker resource.

```json
{"id":"1","method":"start","params":{"request":{"source":"prompt","prompt":"Run task","artifacts":[],"isolationProfile":"isolation.docker@1","providerProfile":"provider.default@1"}}}
{"id":"2","method":"status","params":{}}
{"id":"3","method":"events","params":{}}
{"id":"4","method":"result","params":{}}
{"id":"5","method":"stop","params":{}}
```

Responses are `{ "type": "response", "id", "ok", "result" }` or contain a closed error object.
An `events` subscription also receives `{ "type": "event", "id", "event" }` frames. Arrays,
malformed or oversized JSON, unknown methods, unknown parameters, and duplicate starts fail closed.
EOF, SIGINT, and SIGTERM request explicit stop and wait no longer than the shutdown deadline. They do
not imply rollback.

`result` and `stop` race the in-flight `start` operation against terminal receipt availability. A
startup timeout or allocated-engine start failure therefore returns canonical terminal truth even
when the adapter's original start promise never fulfills.
