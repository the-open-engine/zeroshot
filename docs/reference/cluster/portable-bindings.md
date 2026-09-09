# Portable bindings

Graphs identify executable workers with versioned `WorkerRef` values. Before admission, a backend
resolves each exact reference through `WorkerRegistry` to a `WorkerDescriptor`; there is no
latest-version lookup. The descriptor records the graph profiles and type contract accepted by the
worker, along with artifact limits, opaque credential handles, and an opaque binding identity made
of `protocol`, `version`, and `profile`.

A portable binding sits outside the Cluster Protocol. It translates another worker protocol into a
validated `WorkerOutcome`, while the registry and descriptor checks remain independent of that
protocol. Descriptor fields cannot carry commands, endpoints, credential values, callbacks, or
execution configuration.

## Availability

Zeroshot does not currently ship a portable worker binding. An ACP binding is under development;
there is no supported version, profile identifier, compatibility claim, or production runtime yet.

Native graph nodes use the reserved `openengine.worker.builtin/v1` binding. That binding describes
in-process Zeroshot workers and is not a portable protocol adapter.

## Extension boundary

`WorkerRegistry` is the integration point: an implementation resolves an exact `WorkerRef` or
returns a typed registry error. `WorkerDescriptor` then checks the requested identity, graph
profile, input and output types, verifier contract, artifact policy, and credential handles before
admission. External protocol names and versions are bounded opaque strings; the core protocol does
not keep a catalog of them.

A binding must normalize its terminal result into the closed `WorkerOutcome` algebra. Output,
verifier signals, diagnostics, and artifact receipts must match the resolved descriptor before a
backend records them as verified results. A future binding will define its own profile identity and
translation rules when it ships.
