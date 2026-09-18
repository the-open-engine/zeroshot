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
The optional Cargo `ui` feature embeds `ui/dist`. Release binaries and target images enable it;
Node is needed only at build time. Rebuild the frontend before Rust after UI edits.

Local profiles share the CLI store (`ZEROSHOT_CONFIG_DIR`); ledgers use `ZEROSHOT_STATE_DIR`.
Use temporary absolute directories for isolated tests. Ctrl-C stops the UI while detached runs continue.
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

| Owner                                            | Responsibility                                                                   |
| ------------------------------------------------ | -------------------------------------------------------------------------------- |
| `App.tsx`, `workspace-services.ts`               | Standalone menu/navigation and injected profile, authoring, and history services |
| `WorkflowCanvas`, editor                         | Shared graph presentation, draft/undo, and read-only observation                 |
| `profile_ui.rs`, `profile_ui/{data,outcomes}.rs` | Native catalog, draft transformations, admission, and conditional profile saves  |
| `profile_ui/server.rs`                           | Embedded assets, browser origin checks, and server/observer lifetime             |
| `profile_ui/runs.rs`, `run-history-source.ts`    | Admitted run definitions, paged history, and resumable live observation          |

`/ui/api/data` and `/ui/api/authoring` return drafts. Save validates through native admission and
checks both the content revision and persisted workspace identity. The browser acknowledges only
the saved document generation, preserving newer edits. Layout/theme preferences stay out of profiles.

Run definitions come from admission snapshots. History preserves native event order and opaque
cursors; Rust reconstructs structural visits through its canonical reducer. The browser projects
only the selected prefix. Unavailable/incomplete history remains explicit, with recorded worker
history retained. Observation never starts or recovers a controller.

The history endpoints are `GET /ui/api/runs`, `GET /ui/api/runs/{id}`,
`GET /ui/api/runs/{id}/history?after=CURSOR`, and `GET /ui/api/runs/{id}/events?after=CURSOR`.
Omit `after` to begin. SSE resumes from `Last-Event-ID` and closes after the terminal cursor drains.

## Cloud integration

Local and direct Docker hosts are implemented. [Zero-cloud #301](https://github.com/the-open-engine/zero-cloud/issues/301)
tracks embedding: Zeroshot still needs a shell-less entry point, host bridge, authenticated
run-definition/history export, and structured stream errors. Zero-cloud owns menus/auth,
profile revision checks, and live/archive adapters. The standalone `/ui/api` is not the hosted API.

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
