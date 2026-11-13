# Workspace UI

React Flow and ELK render the graph; Rust owns authoring, admission, storage, and replay.
See [UI setup](../docs/getting-started/install.md#open-the-workspace-ui) for usage and
[run observation](../docs/guides/observe-and-control.md#browser) for live monitoring and replay.

## Build and open

From the repository root:

```sh
npm --prefix ui ci --ignore-scripts
npm --prefix ui run build
cargo run -p zeroshot --features ui -- ui
```

Open `http://127.0.0.1:4173/ui/`; use `--listen 127.0.0.1:PORT` for another loopback port.
Run this loopback server only on a trusted single-user host. It is not an isolation boundary from
other OS users or local processes.
The optional Cargo `ui` feature embeds `ui/dist`. Release binaries and target images enable it;
Node is needed only at build time. Rebuild the frontend before Rust after UI edits.

Local profiles share the CLI store (`ZEROSHOT_CONFIG_DIR`); ledgers use `ZEROSHOT_STATE_DIR`.
Pass `--target NAME` to keep those profiles local while reading history from a configured direct or
hosted target. Hosted credentials stay in the local UI server and never enter the browser. Use
temporary absolute directories for isolated tests. Ctrl-C stops the UI while detached runs continue.
Direct `target serve` mounts `/ui/` on its existing listener, stores profiles/history under
`--storage`, and requires the browser's exact `--public-origin`. Private/hosted targets exclude
this mount. See the [target image guide](../docker/zeroshot-target/README.md) for persistence and restart behavior.

The bootstrap `harnesses` catalog supplies each host-enabled adapter's `id`, `label`, and
`providers`. The editor uses this catalog for runtime choices; `runtimeSchema` supplies structural
fields and effort choices. For older hosts that omit the harness catalog, choices come from their
legacy runtime schema; an explicit empty catalog offers no adapters. Local catalogs include compiled
[contributed adapters](../docs/project/harness-adapters.md); target and Platform catalogs use their
host's enabled registry. Existing unavailable selections remain visible and stay in the profile until
the author changes them.

Each bootstrap template includes its complete authored `runtime`, a concise `description`, and
`requirements`, a map from connection key to sorted field names for contained execution. New profile
documents clone the graph and runtime without flattening explicit node lanes, effort, sessions or
custom connection declarations. Blank profiles keep empty runtime settings. Templates omit size so
the selected target owns its default, and keep workspace checkpoints enabled; the supplied local
Code review runtime remains available separately with its local snapshot opt-out and GitHub-specific
publication connection. Catalog defaults use the native source-neutral delivery binding, so selecting
a graph does not demand GitHub before the run's source or comment destination is selected.

The `setupLanes` array comes from the same harness registry. Each entry contains `harness`, `provider`,
`connectionKey`, `requiredFields`, and `acceptedFieldSets`; a host can offer compatible providers and
request access without maintaining a parallel provider table. `workspace::profile_setup({runtime})`
returns `{runtime, requirements}` after applying native contained-provider access defaults. It reads no
secrets and never saves or runs a profile. Authored custom connections and alternative credential
forms remain intact. `profile_setup_with_harnesses` uses the host-selected registry for validation and
access derivation. Local setup uses its advertised contributed harnesses; target/default setup remains
core-only. Environment and source access stay with run submission.

The bootstrap `runSizes` catalog contains `kind: "openengine.run-sizes/v2"`, a target-owned
`default` identifier, and `sizes: [{id, description}]`. Identifiers are opaque strings of 1–128
non-control Unicode characters; the editor does not maintain or infer size names from the runtime
schema. An empty catalog offers no sizes and has no default. New runtime plans omit `size` so the
target resolves its default at admission;
choosing Target default removes an explicit size rather than copying today's default into the profile.
An explicit `size: null` also uses the target default and stays unchanged through JSON and form
views until the author edits that field.
Unavailable authored selections stay visible and unchanged until the author changes them.

Hosting shells may also supply a `machines` array in the existing bootstrap. Each entry has `size`,
`label`, `vcpus`, `memoryMiB`, `diskGiB`, `laneUnits`, and optional
`gpu: {name, memoryMiB}` or `null`. The shared size picker displays the host label and lane count. Starting a size with Boost uses its full `laneUnits` in Boosts for that run; ordinary
runs do not spend Boosts merely because a size is selected. This is presentation metadata: the host
owns admission and billing. `runSizes` owns offered identifiers when present; matching machine
metadata supplies labels and lane counts. A host providing only machine metadata offers those
identifiers, and an explicit empty list offers none. Without either catalog, the editor accepts a
freeform target-owned identifier or leaves size omitted; it never invents a global size list.
Resource descriptors never enter the saved runtime plan.

Profiles contain graphs and agent runtime configuration. Environments belong to run submission,
not profile authoring. Plain local UI has no environment catalog or management page.

Local profile lists return readable metadata in `profiles` and malformed entries in optional `problems`,
with each entry's name, default status and diagnostic. The sidebar shows these problems alongside
usable profiles. Graph/runtime schemas are checked when a profile is opened; unrelated saves and
metadata operations preserve incompatible entries. Target v1 profile responses retain their original
`profiles`-only contract; diagnostics are local CLI/UI data. Invalid shared store JSON remains a store error.

For frontend development, keep the native server running:

```sh
npm --prefix ui run dev
```

Open `http://127.0.0.1:5173/ui/`. Set `ZEROSHOT_UI_TARGET` on the Vite command to use a different
backend origin. The development proxy adapts its known loopback origin; production serves embedded assets.

## Ownership

| Owner                                                       | Responsibility                                                                  |
| ----------------------------------------------------------- | ------------------------------------------------------------------------------- |
| `App.tsx`, `workspace-services.ts`                          | Shared editor and injected profile, authoring, and history services             |
| `WorkflowCanvas`, editor                                    | Shared graph presentation, draft/undo, and read-only observation                |
| `profile_ui.rs`, `profile_ui/{data,outcomes}.rs`            | Native catalog, draft transformations, admission, and conditional profile saves |
| `profile_ui/server.rs`                                      | Embedded assets, browser origin checks, and server/observer lifetime            |
| `native_v2_observability::history`, `run-history-source.ts` | Admitted definitions, native control projection and resumable observation       |

`/ui/api/data` and `/ui/api/authoring` return drafts. Save validates through native admission and
checks both the content revision and persisted workspace identity. The browser acknowledges only
the saved document generation, preserving newer edits. Layout/theme preferences stay out of profiles.

Run definitions come from admission snapshots. History preserves native event order and opaque
cursors; Rust reconstructs structural visits through its canonical reducer. The browser projects
only the selected prefix. Unavailable/incomplete history remains explicit, with recorded worker
history retained. Observation never starts or recovers a controller.

The history endpoints are `GET /ui/api/runs`, `GET /ui/api/runs/{id}`,
`GET /ui/api/runs/{id}/history?after=CURSOR`, and `GET /ui/api/runs/{id}/events?after=CURSOR`.
These are the local browser BFF routes. A selected target is read server-side through its discovered
`zeroshot.run-history/v1` list/detail/page templates; target URLs and authorization are not browser
contracts.
Omit `after` to begin. SSE resumes from `Last-Event-ID` and closes after the terminal cursor drains.

## Platform integration

Build `ui/dist` from Platform's exact pinned Zeroshot source revision and mount it on the
Platform origin independently of run containers. The shell-less entry is `embed.html`;
`index.html` retains the standalone toolbar. Both entries use the same editor and viewer.
Platform must permit its same-origin frame and the workspace's bundled assets/ELK worker in CSP.

Embedding hosts can call `zeroshot_engine::workspace::{catalog, author, data, profile_setup, validate_profile}`
with Cargo's `workspace` feature; this does not embed UI assets. Authenticated private targets
export native definition/history pages separately from the standalone UI. See the
[public workspace overview](../planning/plans/cloud-workspace-ui-301.md).

### Bridge version 1

`workspace-bridge.ts` defines the wire types. All messages use `postMessage` with the exact
same origin and expected parent/iframe window. The iframe first sends
`{version: 1, type: "ready", requestId}`. Reply with `init` using that request ID:

```json
{
  "version": 1,
  "type": "init",
  "requestId": "ready-request-id",
  "workspaceId": "opaque-user-org-scope-workspace-id",
  "authority": { "userId": "user-id", "organizationId": "org-id", "scope": "user" },
  "apiBase": "/workspace-api/",
  "theme": "light",
  "csrf": { "cookieName": "__Host-workspace-csrf", "headerName": "X-Zeroshot-CSRF" }
}
```

The authenticated bootstrap must return `version: 1` and
`workspace: {kind: "platform", id: workspaceId}`. Bind this ID to user, organization and profile
scope. Recreating the iframe binds a different authority; protect the old draft before doing so.
The `platform` kind starts a new browser storage namespace for drafts and preferences.
Optional CSRF configuration contains names only. The iframe reads the existing cookie for each
service write and rejects missing/duplicate cookies. Platform omits this configuration only for its
server-selected local authentication mode. Credentials never enter messages.

After `init`, every message carries `version`, `workspaceId`, `documentId`, `generation` and
`requestId`. Wait for `state` with `loading: false` before opening a document. Use the latest
state's document ID/generation, including any recovered draft; the initial document ID is null.
The workspace rejects stale document generations. A document switch may change both values.

| Host message           | Payload in addition to the envelope                                                                                           |
| ---------------------- | ----------------------------------------------------------------------------------------------------------------------------- |
| `open_profile`         | `nextDocumentId`, optional `discard`, and either `profile: {name, graph, runtime}` with optional `revision`, or `templateId`. |
| `open_run`             | `nextDocumentId`, `runId`, optional `discard`. History is fetched through the API.                                            |
| `request_save`         | Optional `name` for profile save-as. Captures the current profile or environment draft.                                       |
| `save_ack`             | The snapshot's request ID and generation, with a saved profile/environment or `problem: {code, message, details?}`.           |
| `environment_settings` | Environment view only; reports whether opaque host-owned settings are dirty.                                                  |
| `theme`                | `theme: "light"` or `"dark"`.                                                                                                 |
| `navigate`             | `action: "profiles"`, `"runs"` or `"leave"`, optional `discard`.                                                              |

`templateId: "blank"` creates an empty profile using the existing editor factory; otherwise use a
bootstrap template's `id` (or `name` when it has no ID). Template opens cannot carry a profile or
revision. New/imported full documents omit revision. Platform never constructs graph bindings.

Dirty or unresolved drafts refuse switches with `unsaved_changes`; an explicit host confirmation
may resend with `discard: true`. Pending saves refuse switches even with discard. The host waits
for the correlated `state` acknowledgement before changing outer navigation or removing the frame.

`save_snapshot` replies contain the captured `authority`, the original request/document/generation
envelope, and either `profile: {name, graph, runtime, expectedRevision}` or
`environment: {id?, name, definition, expectedRevision}`. Valid focused field edits commit before
capture; unresolved edits return a structured problem. The host performs the conditional write and
returns `save_ack`. An acknowledgement only settles its captured snapshot, preserving later edits.
A profile save-as name drops the old revision.

Unsolicited `state` messages report `dirty`, `pending`, `validation`, `saving`, `loading` and profile
`name`. Command replies report `accepted` or a structured `problem`. `navigate` messages from the
workspace request the host's `save` or `defaults` action. The host keeps selection and outer menus.

Set `view: "environments"` and an explicit `environmentApi` collection URL in `init` to mount the hosted environment manager;
`readOnly: true` disables mutation controls. The server remains responsible for write authorization.
The manager reuses the same theme, state, and guarded navigation messages, with one document identity
for its mounted surface. It handles resource reads and deletion through the service below. Saves emit
only the generic environment snapshot; the host may combine it atomically with private metadata before
acknowledging it. A single opaque dirty bit brings that metadata into the manager's existing navigation
guard without exposing its shape. Profile-specific bridge commands are unavailable. Failed writes
preserve the draft and saved revision.

### Authenticated service paths and history lifecycle

`apiBase` must be a same-origin absolute path ending with `/`. The embedded workspace uses these
paths beneath it; its profile load/save work passes through the host bridge instead:

| Request                              | Response                                                                                                                     |
| ------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------- |
| `GET environmentApi`                 | `{environments: [{id, name, revision}]}`.                                                                                    |
| `GET environmentApi/{id}`            | `{id, name, definition, revision}`.                                                                                          |
| `POST environmentApi`                | Host save of `{id?, name, definition, expectedRevision: string or null}` plus host-owned fields; returns the saved resource. |
| `DELETE environmentApi/{id}`         | Delete with `{expectedRevision}`; returns `{deleted: true}`.                                                                 |
| `GET bootstrap`                      | Existing workspace bootstrap with templates, workers and runtime schema.                                                     |
| `POST validate`                      | Native validation of `{graph, runtime}`.                                                                                     |
| `POST authoring`, `POST data`        | Native draft transform of `{graph, runtime, action}`.                                                                        |
| `GET runs/{id}`                      | Version 1 admitted definition with projection version 1.                                                                     |
| `GET runs/{id}/history?after=CURSOR` | Existing ordered native history page.                                                                                        |
| `GET runs/{id}/events?after=CURSOR`  | SSE `history` pages and structured `history_error` problems.                                                                 |

Environment writes carry the pinned `X-Zeroshot-Workspace` bootstrap identity. The generic hosted
manager preserves the explicit collection URL's query parameters for reads, deletion, and draft
recovery; the host owns authorization, persistence, and any additional resource fields.
It reports opaque `resourceId`/`resourceRevision` state changes so host controls can refresh their
catalog. Plain local services never create this adapter. Profiles reject `runtime.environment`.
Committed drafts and their original CAS baseline recover in the same authority and collection
context. Save, delete, and explicit discard clear recovery. Storage errors or size overflow warn
that recovery is unavailable; accepted runs retain their already resolved environment snapshot.

The shared fetch SSE reader retains HTTP/stream problem codes and details, reconnects network
interruptions using the last accepted `Last-Event-ID`, and cancels readers/timers on disposal.
Frames have an 8 MiB browser limit. HTTP denials and invalid history stop automatic reconnect.

Definition wrappers and pages may carry
`observation: {state: "active" | "collecting" | "complete" | "incomplete" | "expired" | "unavailable", code?}`.
This metadata describes retained observation; current run termination remains separate.
A `finished` run can still follow `active` or `collecting` observation. Completion closes the stream
only after `page.complete` drains the observed head. Platform retains these execution cursors and native
control records across archive handoff; Platform status cursors never enter graph playback.

## Verification

Start with the UI tests and native service tests:

```sh
npm --prefix ui test
npm --prefix ui run build
cargo test -p zeroshot --features ui profile_ui --lib
```

For native UI/server changes, also run the affected crate with the feature enabled:

```sh
cargo clippy -p zeroshot --all-targets --features ui -- -D warnings
cargo test -p zeroshot --features ui # Unix; Windows: powershell -NoProfile -File scripts/test-windows.ps1 -Ui
```

Default Cargo checks omit `ui`. Browser scenarios and evidence:
[example processes](qa/real-user-processes.md), [replay](qa/replay-real-runs.md),
[live runs](qa/live-run-review.md), and [standalone lifecycle](qa/standalone-integration.md).

Map inspectors label `maxItems` as concurrent items. The runtime editor exposes optional
`maxConcurrency`, with an effective default of 64. Canvas map regions show item concurrency and
only empty-input bypasses. Derived history records expose queued leaves without inventing
execution IDs or worker events; durable node starts remain the authority for running work.
