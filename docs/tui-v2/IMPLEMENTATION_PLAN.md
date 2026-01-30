# Zeroshot TUI v2 (Ratatui) - Architecture + Migration Plan

Date: 2026-01-30
Status: Proposed (supersedes the Ink-only plan)

## Summary

We already implemented an Ink/TypeScript UI under `src/tui/`. We are pivoting to:

- **Frontend:** Rust + Ratatui (rendering + input + layout)
- **Backend:** existing Node/TypeScript code (orchestrator/ledger/providers/etc) exposed via a small local RPC protocol

This keeps the **domain logic** (cluster lifecycle, ledger parsing, guidance delivery) in the existing JS runtime, while making the **UI layer** fast, predictable, and easy to iterate on.

The goal is to make UI changes cheap: a layout tweak should generally touch only Rust `ui/*` code, not orchestration logic.

## Guiding Constraints

- Ratatui is the only renderer/input system for the new TUI UI (no Ink in the hot path).
- Avoid rewriting core orchestration in Rust; treat Node as the source of truth for clusters.
- Keep the UI architecture **screen/component-oriented** with a pure render layer.
- Backend ↔ frontend boundary must be **versioned**, **typed**, and testable.
- Allow an incremental cutover (Ink TUI can remain as a fallback during migration).

## Reuse vs Replace (Final Call)

### Reuse (keep, possibly relocate)

- **All orchestration/runtime code**:
  - `src/orchestrator.js`, `src/ledger.js`, `src/message-bus.js`, providers, settings, id detection, etc.
- **Most of the existing Ink TUI “backend-ish” services** (these are already UI-agnostic and should become the TUI backend implementation):
  - `src/tui/services/cluster-launcher.ts`
  - `src/tui/services/cluster-registry.ts`
  - `src/tui/services/cluster-logs.ts`
  - `src/tui/services/cluster-timeline.ts`
  - `src/tui/services/cluster-topology.ts`
  - `src/tui/services/guidance-delivery.ts`
- Any existing helpers under `lib/` that the services rely on (e.g. `lib/start-cluster.js`).

Net: we keep essentially **all non-UI business logic**.

### Replace (rewrite)

- All Ink rendering and React component code:
  - `src/tui/app.tsx`, `src/tui/views/*`, `src/tui/components/*`, `src/tui/router.tsx`
- Ink-specific navigation plumbing:
  - `src/tui/view-stack.ts`, Ink `useInput` handling, etc.
- Any “UI state helpers” that only exist because of Ink component structure (e.g. pending message UI state).

Net: we replace essentially **all presentation + input plumbing**.

## Proposed Architecture

### High-level diagram

```
zeroshot (Node CLI)
  └─ TTY + no args / `zeroshot tui`:
       exec → zeroshot-tui (Rust, Ratatui)
                ├─ spawns → zeroshot-tui-backend (Node, internal)
                └─ renders → terminal (Ratatui + Crossterm)

zeroshot-tui-backend (Node)
  ├─ orchestrator.create({ quiet: true })
  ├─ reads → ~/.zeroshot/*.db (SQLite ledger)
  ├─ reads → clusters.json (best-effort)
  └─ uses → pidusage (best-effort metrics)
```

### Why this split

- Keeps the orchestrator in a single runtime (Node) to avoid duplicating logic and creating edge-case drift.
- Allows Rust UI to stay “pure”: render from state; run effects to fetch/stream data.
- Makes UI iteration cheap: rework the layout without touching orchestration internals.

## Backend ↔ Frontend Protocol

### Transport

Use a **stdio** connection (Rust spawns Node backend as a child process):

- No ports.
- No firewall concerns.
- Easy to lifecycle-manage (kill backend when UI exits).

Frame messages with a simple length-prefix (LSP-style) to avoid partial reads:

```
Content-Length: <N>\r\n
\r\n
<N bytes of JSON>
```

### Message model

Use JSON-RPC 2.0 semantics:

- Request/response: `{ id, method, params }` → `{ id, result } | { id, error }`
- Backend-to-UI notifications: `{ method, params }` (no id)

### Versioning

- `initialize` request includes `protocolVersion` and client metadata.
- Backend replies with `protocolVersion` and a `capabilities` object.
- Breaking changes require a protocol major bump.

### Core methods (MVP)

Backend methods are intentionally **domain-level** (not layout-level):

- `initialize`
- `listClusters`
- `getClusterSummary` (agents, state, provider, createdAt, messageCount, cwd)
- `listClusterMetrics` (best-effort CPU/mem; `supported=false` if unavailable)
- `startClusterFromText`
- `startClusterFromIssue`
- `sendGuidanceToAgent`
- `sendGuidanceToCluster`
- `subscribeClusterLogs` → notifications `clusterLogLines`
- `subscribeClusterTimeline` → notifications `clusterTimelineEvents`
- `getClusterTopology`

Nice-to-haves (follow-up):

- `stopCluster`, `killCluster`
- `listAgents` / `getAgentSummary`
- `subscribeAgentLogs` (if we want separate stream vs filter in UI)

### Backpressure and bounded memory

- UI keeps a ring buffer per log view (e.g. last 400–2000 lines).
- Backend batches notifications (e.g. up to 50 lines / 250ms) to reduce overhead.
- Backend may send `droppedCount` when UI can’t keep up.

## Rust Ratatui Frontend Architecture

### Key design goal: UI iteration should not rewrite core logic

Adopt a Model-View-Update style with explicit effects:

- `AppState` is pure data.
- `Action` represents inputs/events.
- `update(state, action) -> (state, effects)` is pure.
- `render(frame, state)` is pure (no IO).
- Effects perform backend calls and feed results back as actions.

This keeps UI changes localized to `ui/*` (render) and `screens/*` (state + reducers).

### Proposed crate layout

- `tui-rs/` (Rust workspace)
  - `crates/zeroshot-tui/`
    - `src/main.rs` (terminal init/restore guard, bootstrap)
    - `src/app/mod.rs` (`AppState`, `Action`, `Effect`, `update`)
    - `src/screens/launcher.rs`
    - `src/screens/monitor.rs`
    - `src/screens/cluster.rs`
    - `src/screens/agent.rs`
    - `src/ui/mod.rs` (screen rendering entry)
    - `src/ui/widgets/*` (log viewer, list, status bar, input, toast)
    - `src/input.rs` (keymaps → actions; single source of truth)
    - `src/backend/mod.rs` (`BackendClient` trait)
    - `src/backend/stdio.rs` (Node backend child-process client)
    - `src/protocol/*` (serde types for requests/responses/notifications)

### Navigation model

Keep the “Esc pops stack” behavior, but make it UI-agnostic:

- `ScreenId` enum (`Launcher`, `Monitor`, `Cluster { id }`, `Agent { id }`)
- `Vec<ScreenId>` stack owned by `AppState`
- Navigation actions: `Push(screen)`, `Pop`, `ReplaceTop(screen)`

### Concurrency model

- One UI thread (render loop) driven by:
  - terminal events (key/resize)
  - tick events (e.g. 100–250ms)
  - backend notifications (logs/timeline)
- Use channels (`tokio::mpsc` or std) to merge event sources into `Action`.

### Terminal safety

- Always restore terminal state on exit:
  - raw mode off
  - alternate screen off (if used)
  - cursor restored
- Use a guard object (`Drop`) so panics still restore the terminal.

## TypeScript Backend Architecture

### Proposed code organization

Introduce a backend module that is explicitly _not_ a UI:

- `src/tui-backend/`
  - `server.ts` (stdio JSON-RPC server)
  - `protocol/*` (TS runtime validation + types)
  - `services/*` (moved from `src/tui/services/*` with minimal changes)
  - `subscriptions/*` (log/timeline stream management)
- `lib/tui-backend/` (compiled output shipped in npm package)

During migration, keep the existing Ink UI in place (or moved to `src/tui-ink/`) so we can cut over safely.

### Backend implementation notes

- Cache a single orchestrator instance per backend process (current TUI services already do this).
- Implement subscriptions by wrapping existing `createClusterLogStream` / `createClusterTimelineStream`:
  - each subscription owns the timer + ledger handle
  - on stop/unsubscribe, close ledger + clear interval
- Validate all incoming params (fail fast with structured RPC errors).

## Build, Packaging, and Distribution

### Development workflow (source)

- `cargo run` from `tui-rs/` should “just work”.
- Rust TUI spawns Node backend via `node <path-to-lib/tui-backend/server.js>`.
- For local iteration, we should not require an npm publish; `npm link` remains fine.

### NPM publish strategy (recommended)

We want `npm i -g @covibes/zeroshot` to work without requiring `cargo` on user machines.

Preferred approach:

1. CI builds `zeroshot-tui` binaries for macOS + Linux (x64 + arm64 as needed).
2. Publish binaries as release artifacts.
3. NPM package includes an install script that downloads the correct binary (esbuild-style).

Fallback approach (acceptable if we accept the tradeoff):

- Require `cargo` on install and build from source in `postinstall`.

## Migration Plan (Milestones)

Each milestone should be a tight PR with clear rollback.

### Milestone 0: Stabilize the boundary (TS backend first)

Goal: get a testable backend API without changing user-facing behavior.

- Create `src/tui-backend/*` and move/rewire the reusable services.
- Implement stdio JSON-RPC server with `initialize` + a couple methods (`listClusters`, `startClusterFromText`).
- Add integration tests that spawn the backend and exercise the protocol.
- Keep Ink TUI unchanged for now.

### Milestone 1: Rust “Hello UI” + backend handshake

Goal: prove terminal handling + IPC lifecycle is solid.

- Add Rust crate skeleton with ratatui + crossterm init/restore guard.
- Implement backend child-process client:
  - spawn Node backend
  - send `initialize`
  - render a minimal screen with backend status

### Milestone 2: Rebuild core flows in Rust (minimal layout)

Goal: feature parity with the _flows_, not visuals.

- Launcher: input box, start cluster from text, open Cluster screen.
- Monitor: list clusters + selection + open Cluster screen.
- Cluster: log tail + agent list + timeline (no fancy topology layout yet).
- Agent: agent log tail + send guidance.

### Milestone 3: Fill in “v2” features

- Topology (render from `getClusterTopology`).
- Metrics (poll `listClusterMetrics`).
- Guidance delivery UX (queued vs injected feedback).
- Slash command parity incrementally (either:
  - parse commands in Rust; OR
  - forward raw `/...` to backend `executeCommand` method and return a domain “intent”).

### Milestone 4: Cutover + cleanup

- Switch `zeroshot` (TTY + no args) and `zeroshot tui` to launch the Rust TUI by default.
- Keep `ZEROSHOT_TUI=ink` fallback for one release.
- Remove Ink UI and dependencies once stable:
  - `ink`, `react`, `@types/react`, `tsconfig.tui.json` (or repurpose for backend build)
- Update docs + help text (`zeroshot watch` behavior, etc).

## Detailed Issue Plan (Issue-by-Issue)

These issues are ordered for implementation. Each issue is intended to be a tight PR with a clear rollback point.

### Issue 1 — Protocol Spec + Shared Types (v0)

**Goal:** Define the stable, versioned boundary between the Ratatui UI and Node backend.

**Scope**

- Define JSON-RPC framing (`Content-Length`) and message shapes.
- Specify protocol version negotiation (`initialize`).
- Define MVP request/response/notification types in a canonical spec section.
- Implement TS runtime validation schemas (zod or existing validation style).
- Implement Rust `serde` types mirroring the spec.

**Acceptance Criteria**

- Spec section exists in this doc (or sibling protocol doc) and is versioned.
- TS validation rejects malformed params with structured RPC errors.
- Rust types compile and round-trip serialize/deserialize in tests.

**Dependencies:** none.

---

### Issue 2 — TUI Backend Skeleton + Service Migration

**Goal:** Create the Node backend process and relocate reusable services.

**Scope**

- Add `src/tui-backend/` with `server.ts`, `protocol/*`, `services/*`, `subscriptions/*`.
- Move or re-export existing services from `src/tui/services/*` into backend services.
- Ensure backend starts a single orchestrator instance (quiet mode).
- Build output to `lib/tui-backend/`.

**Acceptance Criteria**

- Backend process boots and stays alive with no UI.
- Services compile and are importable from `src/tui-backend/services/*`.
- No behavior changes to existing CLI flows.

**Dependencies:** Issue 1.

---

### Issue 3 — JSON-RPC Server + Initialize Handshake

**Goal:** Implement the stdio JSON-RPC server with version negotiation.

**Scope**

- Implement stdio framing, request parsing, response writing.
- Implement `initialize` method with client metadata and protocol/capabilities response.
- Structured error handling for unknown methods and bad params.
- Add a simple `ping` or `health` method for diagnostics.

**Acceptance Criteria**

- Backend can be spawned and responds to `initialize`.
- Invalid frames and malformed JSON produce clean RPC errors.
- Protocol version mismatch returns actionable error.

**Dependencies:** Issues 1–2.

---

### Issue 4 — Cluster Listing + Summary APIs

**Goal:** Expose baseline cluster discovery for Launcher/Monitor.

**Scope**

- Implement `listClusters` and `getClusterSummary`.
- Include: id, status, provider, createdAt, cwd, agents count, messageCount.
- Best-effort read from orchestrator registry + clusters.json.

**Acceptance Criteria**

- TUI backend returns a stable list for empty/non-empty registries.
- Summaries are consistent with existing `zeroshot status`.

**Dependencies:** Issue 3.

---

### Issue 5 — Start Cluster from Text / Issue

**Goal:** Enable TUI to launch clusters using existing CLI paths.

**Scope**

- Implement `startClusterFromText` (free-form input).
- Implement `startClusterFromIssue` (supports issue ref parsing).
- Ensure provider override can be supplied per request (session-scoped).

**Acceptance Criteria**

- Launching via API is equivalent to CLI `zeroshot run` input.
- Provider override does not persist to settings.
- Errors bubble to RPC responses with actionable messages.

**Dependencies:** Issue 4.

---

### Issue 6 — Log + Timeline Subscriptions

**Goal:** Stream live logs and timeline events to the UI.

**Scope**

- Implement `subscribeClusterLogs` (notifications: `clusterLogLines`).
- Implement `subscribeClusterTimeline` (notifications: `clusterTimelineEvents`).
- Batch notifications (e.g., 50 lines / 250ms) with `droppedCount`.
- Implement unsubscribe/cleanup to avoid leaks.

**Acceptance Criteria**

- Log stream tails without unbounded memory.
- Timeline events appear in order and dedupe correctly.
- Unsubscribe stops ledger polling and releases resources.

**Dependencies:** Issue 4.

---

### Issue 7 — Guidance Delivery APIs

**Goal:** Allow cluster/agent guidance from the TUI with status feedback.

**Scope**

- Implement `sendGuidanceToCluster` and `sendGuidanceToAgent`.
- Return delivery status (injected vs queued) plus reason when queued.
- Wire into existing guidance-delivery service and mailbox logic.

**Acceptance Criteria**

- Guidance is delivered or queued with status surfaced to caller.
- Provider limitations are represented in structured response fields.

**Dependencies:** Issue 2.

---

### Issue 8 — Topology + Metrics APIs

**Goal:** Expose topology and resource metrics for Monitor/Cluster views.

**Scope**

- Implement `getClusterTopology` (MVP adjacency/agent list).
- Implement `listClusterMetrics` (CPU/mem best-effort; supported flag).
- Graceful degradation when metrics unsupported.

**Acceptance Criteria**

- Topology output is stable enough to render.
- Metrics calls never crash the backend; `supported=false` when unavailable.

**Dependencies:** Issue 4.

---

### Issue 9 — Rust TUI Crate Skeleton + Terminal Safety

**Goal:** Bootstrap the Ratatui app with safe terminal lifecycle handling.

**Scope**

- Add `tui-rs/` workspace with `crates/zeroshot-tui`.
- Implement terminal init/restore guard (raw mode, alt screen, cursor).
- Add event loop with tick + resize + input events.

**Acceptance Criteria**

- `cargo run` opens a blank UI and exits without corrupting terminal.
- Panics still restore terminal state.

**Dependencies:** Issue 1.

---

### Issue 10 — Backend Client (stdio) + Handshake

**Goal:** Connect Rust UI to the Node backend over stdio.

**Scope**

- Spawn backend child process from Rust.
- Implement framing, request/response, and notification handling.
- Perform `initialize` handshake and display status.

**Acceptance Criteria**

- Rust UI connects to backend and prints/reflects protocol capabilities.
- Backend process is terminated when UI exits.

**Dependencies:** Issues 3, 9.

---

### Issue 11 — Core App Architecture (State/Action/Effects)

**Goal:** Implement the MVU-style core with navigation stack.

**Scope**

- Define `AppState`, `Action`, `Effect`, `update`, `render`.
- Implement screen stack (`Launcher`, `Monitor`, `Cluster`, `Agent`).
- Implement global input handling and keymap routing.

**Acceptance Criteria**

- Esc consistently pops screen stack.
- Inputs route to current screen without leaking state across screens.

**Dependencies:** Issue 9.

---

### Issue 12 — Launcher Screen (Text + Commands)

**Goal:** Make the primary launcher flow functional.

**Scope**

- Central input box + hint text.
- Non-`/` input launches `startClusterFromText`.
- `/` input routes to command parser (MVP commands wired).
- On launch success, transition to Cluster screen.

**Acceptance Criteria**

- Free-text launch works end-to-end.
- Numeric input (`123`) is treated as text (per decision).

**Dependencies:** Issues 5, 11.

---

### Issue 13 — Monitor Screen (Cluster List)

**Goal:** Provide high-level view of all clusters.

**Scope**

- List clusters with key fields (status, provider, duration, last activity).
- Poll `listClusters` on interval.
- Up/Down selection + Enter opens Cluster screen.
- Esc returns to previous screen.

**Acceptance Criteria**

- Monitor list renders and updates without jitter.
- Enter opens selected cluster reliably.

**Dependencies:** Issues 4, 11.

---

### Issue 14 — Cluster Screen (Logs + Agents + Timeline)

**Goal:** Provide focused cluster monitoring.

**Scope**

- Multi-pane layout: logs, agents list, timeline (topology placeholder).
- Log ring buffer, per-agent attribution.
- Tab/Left/Right to cycle focus across panes.
- Enter on agent opens Agent screen.

**Acceptance Criteria**

- Logs stream live and maintain bounded memory.
- Focus and selection behave consistently.

**Dependencies:** Issues 6, 11.

---

### Issue 15 — Agent Screen (Focused Logs + Guidance Input)

**Goal:** Drill into a single agent and send guidance.

**Scope**

- Filtered log view for agent.
- Agent identity + status display.
- Guidance input box wired to `sendGuidanceToAgent`.

**Acceptance Criteria**

- Guidance delivery status is shown (queued vs injected).
- Esc returns to Cluster screen.

**Dependencies:** Issues 7, 14.

---

### Issue 16 — Slash Command MVP

**Goal:** Implement MVP command set and global command bar.

**Scope**

- Commands: `/help`, `/monitor`, `/issue <ref>`, `/provider <name>`, `/quit` (`/exit`).
- Shared parsing + dispatch (single source of truth).
- Lightweight toast/output area for command results.

**Acceptance Criteria**

- Commands work consistently across screens.
- Invalid commands produce readable errors without crashing UI.

**Dependencies:** Issues 5, 11–13.

---

### Issue 17 — Topology Rendering (MVP)

**Goal:** Render the cluster topology in the Cluster screen.

**Scope**

- Use `getClusterTopology` data.
- MVP ASCII/adjacency layout with stable ordering.
- Integrate into existing pane layout.

**Acceptance Criteria**

- Topology renders for existing templates without overlaps or crashes.
- Missing topology data degrades to a friendly placeholder.

**Dependencies:** Issues 8, 14.

---

### Issue 18 — Metrics Display (Monitor + Cluster)

**Goal:** Surface CPU/memory where available.

**Scope**

- Poll `listClusterMetrics` on a slower cadence (e.g., 2s).
- Display per-cluster aggregate metrics in Monitor.
- Display per-agent metrics or aggregate in Cluster.

**Acceptance Criteria**

- When `supported=false`, UI shows “—” and remains stable.
- Metrics refresh does not cause UI stutter.

**Dependencies:** Issue 8.

---

### Issue 19 — CLI Wiring + Entry Points

**Goal:** Make new TUI accessible from existing commands.

**Scope**

- `zeroshot` (no args) launches Rust TUI when TTY.
- `zeroshot tui` always launches TUI.
- `zeroshot watch` opens Monitor screen.
- `zeroshot codex|claude|gemini|opencode` sets session provider override.
- `ZEROSHOT_TUI=ink` fallback retained for one release.

**Acceptance Criteria**

- CLI behavior matches PRD in TTY and non-TTY contexts.
- Provider override applies only to the TUI session.

**Dependencies:** Issues 9–16.

---

### Issue 20 — Packaging + Distribution

**Goal:** Ship Rust TUI without requiring cargo on user machines.

**Scope**

- CI build for macOS/Linux (x64 + arm64 as needed).
- Add install script in npm package to download correct binary.
- Ensure `zeroshot` wrapper locates and launches bundled binary.

**Acceptance Criteria**

- `npm i -g @covibes/zeroshot` installs and runs TUI without cargo.
- Manual override for local dev (`cargo run`) still works.

**Dependencies:** Issue 19.

---

### Issue 21 — Cutover + Cleanup

**Goal:** Fully replace Ink UI and remove legacy dependencies.

**Scope**

- Switch default entrypoints to Rust TUI.
- Remove Ink UI code and deps after stabilization.
- Update docs, help text, and `zeroshot watch` description.

**Acceptance Criteria**

- No Ink deps remain in package graph.
- All docs refer to Ratatui TUI as default.

**Dependencies:** Issues 19–20.

---

### Issue 22 — Final Design Touch-Up (Iterative with Product)

**Goal:** Refine layout, spacing, typography, and interaction polish based on user feedback.

**Scope (lightweight by design)**

- Iterate on UI layout in `ui/*` and `screens/*`.
- Tune keybindings and command bar ergonomics.
- Adjust visual hierarchy and spacing for readability.

**Acceptance Criteria**

- Agreed-upon polish pass is shipped.
- No architecture changes required; purely UI-level iteration.

**Dependencies:** Issues 14–21.

## Deferred: UI Layout Reflection (When We’re Ready)

We will iterate on layout after the architecture is stable. The design constraints to keep in mind:

- Prefer a **stable global command/input bar** across screens (muscle memory).
- Treat panes as **widgets** with independent state (log viewer, list, topology view).
- Keep all layout in `ui/*` so we can redesign without touching reducers/backend calls.
