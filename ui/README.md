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

Local profiles and reusable environments share the CLI store (`ZEROSHOT_CONFIG_DIR`);
ledgers use `ZEROSHOT_STATE_DIR`. Profiles reference environments by ID. New runs resolve the latest
saved definition; accepted runs and resumes retain their captured definition.
Pass `--target NAME` to keep those profiles local while reading history from a configured direct or
hosted target. Hosted credentials stay in the local UI server and never enter the browser. Use
temporary absolute directories for isolated tests. Ctrl-C stops the UI while detached runs continue.
Direct `target serve` mounts `/ui/` on its existing listener, stores profiles/history under
`--storage`, and requires the browser's exact `--public-origin`. Private/hosted targets exclude
this mount. See the [target image guide](../docker/zeroshot-target/README.md) for persistence and restart behavior.

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

## Cloud integration

Build `ui/dist` from Cloud's exact pinned Zeroshot source revision and mount it on the
Cloud origin independently of run containers. The shell-less entry is `embed.html`;
`index.html` retains the standalone toolbar. Both entries use the same editor and viewer.
Cloud must permit its same-origin frame and the workspace's bundled assets/ELK worker in CSP.

The API-gateway can call `zeroshot_engine::workspace::{catalog, author, data, validate_profile}`
with Cargo's `workspace` feature; this does not embed UI assets. Authenticated private targets
export native definition/history pages separately from the standalone UI. See the
[approved implementation plan](../planning/plans/cloud-workspace-ui-301.md).

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
  "apiBase": "/_bff/orgs/org-id/workspace/user/",
  "theme": "light",
  "csrf": { "cookieName": "__Host-zsc-csrf", "headerName": "X-Zeroshot-CSRF" }
}
```

The authenticated bootstrap must return `version: 1` and
`workspace: {kind: "cloud", id: workspaceId}`. Bind this ID to user, organization and profile
scope. Recreating the iframe binds a different authority; protect the old draft before doing so.
Optional CSRF configuration contains names only. The iframe reads the existing cookie for each
service write and rejects missing/duplicate cookies. Cloud omits this configuration only for its
server-selected local authentication mode. Credentials never enter messages.

After `init`, every message carries `version`, `workspaceId`, `documentId`, `generation` and
`requestId`. Wait for `state` with `loading: false` before opening a document. Use the latest
state's document ID/generation, including any recovered draft; the initial document ID is null.
The workspace rejects stale document generations. A document switch may change both values.

| Host message   | Payload in addition to the envelope                                                                                           |
| -------------- | ----------------------------------------------------------------------------------------------------------------------------- |
| `open_profile` | `nextDocumentId`, optional `discard`, and either `profile: {name, graph, runtime}` with optional `revision`, or `templateId`. |
| `open_run`     | `nextDocumentId`, `runId`, optional `discard`. History is fetched through the API.                                            |
| `request_save` | Optional `name` for save-as. The current identity must be a profile.                                                          |
| `save_ack`     | The snapshot's request ID and generation, with either `saved: {profile, revision}` or `problem: {code, message, details?}`.   |
| `theme`        | `theme: "light"` or `"dark"`.                                                                                                 |
| `navigate`     | `action: "profiles"`, `"runs"` or `"leave"`, optional `discard`.                                                              |

`templateId: "blank"` creates an empty profile using the existing editor factory; otherwise use a
bootstrap template's `id` (or `name` when it has no ID). Template opens cannot carry a profile or
revision. New/imported full documents omit revision. Cloud never constructs graph bindings.

Dirty or unresolved drafts refuse switches with `unsaved_changes`; an explicit host confirmation
may resend with `discard: true`. Pending saves refuse switches even with discard. The host waits
for the correlated `state` acknowledgement before changing outer navigation or removing the frame.

`save_snapshot` replies contain `profile: {name, graph, runtime, expectedRevision}`, the captured
`authority`, and the original request/document/generation envelope. Valid focused field edits commit
before capture; unresolved edits return a structured problem. The host performs the conditional
write with the captured authority/workspace/revision and returns `save_ack`. An acknowledgement
only settles its captured snapshot, preserving later edits. A save-as name drops the old revision.

Unsolicited `state` messages report `dirty`, `pending`, `validation`, `saving`, `loading` and profile
`name`. Command replies report `accepted` or a structured `problem`. `navigate` messages from the
workspace request the host's `save`, `defaults`, or `environments` action. The host keeps selection and outer menus.

Set optional `view: "environments"` in `init` to mount the independent environment manager;
`readOnly: true` disables mutation controls. The server remains responsible for write authorization.
The manager reuses the same theme, state, and guarded navigation messages, with one document identity
for its mounted surface. It handles resource selection and conditional CRUD through the service below;
profile-specific bridge commands are unavailable. Failed writes preserve the draft and saved revision.
Committed environment edits also recover from session storage after Back/Forward or remount, scoped
to the same origin, mount, workspace kind, and workspace identity as profile drafts. Recovery retains
the original CAS revision. Saving, deleting, or explicitly discarding clears that recovered draft.

### Authenticated service paths and history lifecycle

`apiBase` must be a same-origin absolute path ending with `/`. The embedded workspace uses these
paths beneath it; its profile load/save work passes through the host bridge instead:

| Request                              | Response                                                                 |
| ------------------------------------ | ------------------------------------------------------------------------ |
| `GET environments` | `{environments: [{id, name, revision}]}`. |
| `GET environments/{id}` | `{id, name, definition, revision}`. |
| `POST environments` | Save `{id?, name, definition, expectedRevision: string or null}`; returns the saved resource. |
| `DELETE environments/{id}` | Delete with `{expectedRevision}`; returns `{deleted: true}`. |
| `GET bootstrap`                      | Existing workspace bootstrap with templates, workers and runtime schema. |
| `POST validate`                      | Native validation of `{graph, runtime}`.                                 |
| `POST authoring`, `POST data`        | Native draft transform of `{graph, runtime, action}`.                    |
| `GET runs/{id}`                      | Version 1 admitted definition with projection version 1.                 |
| `GET runs/{id}/history?after=CURSOR` | Existing ordered native history page.                                    |
| `GET runs/{id}/events?after=CURSOR`  | SSE `history` pages and structured `history_error` problems.             |

Environment writes carry the pinned `X-Zeroshot-Workspace` bootstrap identity. Referenced
environments cannot be deleted. Profile runtimes carry only `environment: {id}`; the environment
manager owns scripts, variables, and preparation connection declarations.

The shared fetch SSE reader retains HTTP/stream problem codes and details, reconnects network
interruptions using the last accepted `Last-Event-ID`, and cancels readers/timers on disposal.
Frames have an 8 MiB browser limit. HTTP denials and invalid history stop automatic reconnect.

Definition wrappers and pages may carry
`observation: {state: "active" | "collecting" | "complete" | "incomplete" | "expired" | "unavailable", code?}`.
This metadata describes retained observation; current run termination remains separate.
A `finished` run can still follow `active` or `collecting` observation. Completion closes the stream
only after `page.complete` drains the observed head. Cloud retains these execution cursors and native
control records across archive handoff; Cloud status cursors never enter graph playback.

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
