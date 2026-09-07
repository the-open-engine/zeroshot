# Legacy ship worker protocol fixture

`legacy.zeroshot.ship@1` is retained only as a closed protocol fixture used by Rust schema,
validation, and conformance tests. Zeroshot v8 does not ship the former JavaScript worker binding,
its executable, or a compatibility adapter for this protocol.

## Closed input

`LegacyShipRequest` selects exactly one source:

- `issue`: nonempty `issue`, no prompt, and no artifacts;
- `prompt`: nonempty `prompt`, no issue, and no artifacts;
- `artifact`: one or more byte-free `ArtifactRef` receipts and no issue or prompt.

`isolationProfile` and `providerProfile` are opaque registry handles. Unknown fields are rejected, so
payloads cannot carry credentials, tokens, environment maps, endpoints, models, timeouts, filesystem
paths, commands, or launch flags.

## Closed result

The lifecycle vocabulary is `idle`, `starting`, `running`, optional `stopping`, then exactly one of
`completed`, `failed`, `timed_out`, `stopped`, or `malformed`. Results and errors are closed bounded
values. Artifact references are byte-free receipts; raw provider output is never a stable result.

## Fixture framing

The archived NDJSON fixture vocabulary contains `start`, `status`, `events`, `result`, and `stop`
requests. Responses are `{ "type": "response", "id", "ok", "result" }` or contain a closed error
object. Event fixtures use `{ "type": "event", "id", "event" }`.

These values document historical wire fixtures only. They are not accepted by the v8 CLI or target
server and must not be used to reconstruct a removed runtime surface.
