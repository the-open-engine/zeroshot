# Zeroshot TUI v2 (Ratatui) - Architecture + Migration Plan

Date: 2026-01-30
Status: Completed (Ratatui-only; legacy UI removed)

## Summary

The legacy TypeScript UI has been removed. The Ratatui UI is now the only TUI:

- **Frontend:** Rust + Ratatui (rendering + input + layout)
- **Backend:** existing Node/TypeScript code (orchestrator/ledger/providers/etc) exposed via a small local RPC protocol

This keeps the **domain logic** (cluster lifecycle, ledger parsing, guidance delivery) in the existing JS runtime, while making the **UI layer** fast, predictable, and easy to iterate on.

The goal is to make UI changes cheap: a layout tweak should generally touch only Rust `ui/*` code, not orchestration logic.

## Guiding Constraints

- Ratatui is the only renderer/input system for the new TUI (no legacy UI in the hot path).
- Avoid rewriting core orchestration in Rust; treat Node as the source of truth for clusters.
- Keep the UI architecture **screen/component-oriented** with a pure render layer.
- Backend ↔ frontend boundary must be **versioned**, **typed**, and testable.

## Reuse vs Replace (Final Call)

### Reuse (keep, possibly relocate)

- **All orchestration/runtime code**:
  - `src/orchestrator.js`, `src/ledger.js`, `src/message-bus.js`, providers, settings, id detection, etc.
- **TUI backend services** (UI-agnostic and now the backend implementation):
  - `src/tui-backend/services/cluster-launcher.ts`
  - `src/tui-backend/services/cluster-registry.ts`
  - `src/tui-backend/services/cluster-logs.ts`
  - `src/tui-backend/services/cluster-timeline.ts`
  - `src/tui-backend/services/cluster-topology.ts`
  - `src/tui-backend/services/guidance-delivery.ts`
- Any existing helpers under `lib/` that the services rely on (e.g. `lib/start-cluster.js`).

Net: we keep essentially **all non-UI business logic**.

### Replace (rewrite)

- Legacy UI rendering and React component code (removed during cutover).
- Legacy UI navigation plumbing (removed during cutover).
- Legacy UI-only state helpers (removed during cutover).

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
  - `subscriptions/*` (log/timeline stream management)
- `lib/tui-backend/` (compiled output shipped in npm package)

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

Asset naming + mapping:

- Asset name: `zeroshot-tui-{platform}-{arch}.tar.gz`
- Platform/arch mapping:
  - `darwin/x64` → `zeroshot-tui-darwin-x64.tar.gz`
  - `darwin/arm64` → `zeroshot-tui-darwin-arm64.tar.gz`
  - `linux/x64` → `zeroshot-tui-linux-x64.tar.gz`
  - `linux/arm64` → `zeroshot-tui-linux-arm64.tar.gz`

Install-time overrides:

- `ZEROSHOT_TUI_BINARY_PATH`: copy a local binary into `libexec/zeroshot-tui` (CI/offline)
- `ZEROSHOT_TUI_BINARY_URL`: override the release asset URL
- `ZEROSHOT_TUI_BINARY_SKIP`: skip download (truthy values)

- Require `cargo` on install and build from source in `postinstall`.

## Migration Plan (Milestones)

Each milestone should be a tight PR with clear rollback.

### Milestone 0: Stabilize the boundary (TS backend first)

Goal: get a testable backend API without changing user-facing behavior.

- Create `src/tui-backend/*` and move/rewire the reusable services.
- Implement stdio JSON-RPC server with `initialize` + a couple methods (`listClusters`, `startClusterFromText`).
- Add integration tests that spawn the backend and exercise the protocol.
- Keep legacy UI TUI unchanged for now.

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
- Remove legacy UI and dependencies once stable:
  - Remove legacy UI dependencies: `react`, `@types/react`, `tsconfig.tui.json` (or repurpose for backend build)
- Update docs + help text (`zeroshot watch` behavior, etc).

## Detailed Issue Plan (Issue-by-Issue)

These issues are ordered for implementation. Each issue is intended to be a tight PR with a clear rollback point.

### Issue Template (for TUI v2 plan work)

Use this template when creating GitHub issues from this plan. Keep each issue exhaustive and self-contained.

- Title format: `[FEATURE] TUI v2: <short description>` (use `[BUG]` or `[DOC]` when applicable)
- Labels: pick one primary label (`enhancement`, `bug`, `documentation`) and add others only if needed
- Include file paths, method names, and explicit acceptance criteria

<details><summary>Issue template (copy/paste)</summary>

```markdown
Goal: <single sentence describing the outcome>

<details><summary>Problem</summary>
- <why this is needed / what is missing today>
</details>

<details><summary>Spec (if applicable)</summary>
- <protocol or behavior spec details>
</details>

<details><summary>Scope</summary>
- In scope: <bulleted list>
- Out of scope: <bulleted list>
</details>

<details><summary>Implementation Plan</summary>
1. <concrete step with file paths>
2. <concrete step with file paths>
3. <concrete step with file paths>
</details>

<details><summary>Testing</summary>
- <unit/integration/manual checks>
</details>

<details><summary>Acceptance Criteria</summary>
- <verifiable, binary outcomes>
</details>

<details><summary>Alternatives</summary>
- <option A> (pros/cons)
- <option B> (pros/cons)
</details>

<details><summary>Dependencies</summary>
- <issue numbers or prerequisite work>
</details>
```

</details>

### Issue 1 — Protocol Spec + Shared Types (v0)

Status: Completed (Issue #240, closed 2026-01-31)

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

Status: Completed (Issue #241, closed 2026-01-31)

**Goal:** Create the Node backend process and relocate reusable services.

**Scope**

- Add `src/tui-backend/` with `server.ts`, `protocol/*`, `services/*`, `subscriptions/*`.
- Ensure backend starts a single orchestrator instance (quiet mode).
- Build output to `lib/tui-backend/`.

**Acceptance Criteria**

- Backend process boots and stays alive with no UI.
- Services compile and are importable from `src/tui-backend/services/*`.
- No behavior changes to existing CLI flows.

**Dependencies:** Issue 1.

---

### Issue 3 — JSON-RPC Server + Initialize Handshake

Status: Completed (Issue #242, closed 2026-01-31)

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

Status: Open (Issue #249)

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

Status: Open (Issue #250)

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

Status: Open (Issue #251)

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

**Goal:** Fully replace legacy UI and remove legacy dependencies.

**Scope**

- Switch default entrypoints to Rust TUI.
- Remove legacy UI code and deps after stabilization.
- Update docs, help text, and `zeroshot watch` description.

**Acceptance Criteria**

- No legacy UI deps remain in package graph.
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

## UI Polish Issues (Post-Architecture)

Architecture is stable. These issues focus on visual design — all changes are in `ui/` and render functions in `screens/`. No backend or state management changes required.

Design reference: Catppuccin Mocha palette, inspired by lazygit/k9s/gitui aesthetics.

---

### Issue 23 — Theme Module + Color Palette

Status: Open

**Goal:** Centralize all colors/styles into a reusable theme module.

**Scope**

- Create `src/ui/theme.rs` with Catppuccin Mocha-inspired palette:
  - Accent: `#89b4fa` (blue), Accent2: `#a6e3a1` (green)
  - FG primary: `#cdd6f4`, FG dim: DarkGray, FG muted: `#6c7086`
  - Focus border: `#89b4fa`, Unfocus border: `#45475a`
  - Status colors: running=Green, done=`#a6e3a1`, error=`#f38ba8`, pending=Yellow
  - Agent colors (rotating): blue, green, yellow, mauve, teal, flamingo
- Export as `const Style` values and helper functions (`status_style(state)`, `agent_color(index)`)
- Refactor all render functions to import from theme instead of inline `Style::default().fg(...)`

**Acceptance Criteria**

- Zero inline color definitions in render code (all from `theme::`)
- `cargo test` passes (no functional changes)

**Dependencies:** None (can start immediately).

---

### Issue 24 — Compact Global Chrome (8 → 2 lines)

Status: Open

**Goal:** Reclaim 6 vertical lines by compressing header, toast, and command bar.

**Scope**

- Change layout constraints from `[Length(2), Min(1), Length(3), Length(3)]` to `[Length(1), Min(1), Length(1)]`
- **Header (1 line):** `◆ ZEROSHOT  <ScreenTitle>` left-aligned (accent+bold), backend status dot + provider right-aligned
- **Status bar (1 line):** Context-sensitive keyboard hints left-aligned (key in accent, description in dim), toast message right-aligned (fades after 5s)
- When command bar is active (`/` pressed): status bar becomes full-width command input with accent prefix
- Breadcrumb navigation for nested screens: `Monitor > cluster-abc > agent-worker`

**Files:** `ui/mod.rs`, `ui/widgets/command_bar.rs`, `ui/widgets/toast.rs`

**Acceptance Criteria**

- Header fits in 1 line with all info visible
- Toast messages appear inline in status bar and auto-dismiss
- Command bar replaces status bar when active
- 6 more lines available for screen content

**Dependencies:** Issue 23.

---

### Issue 25 — Launcher Screen Redesign

Status: Open

**Goal:** Transform empty black screen into a modern command launcher.

**Scope**

- Vertically and horizontally center content (max 60 cols or 70% width)
- **Logo block (2 lines):** `◆ Z E R O S H O T` in accent+bold, `Multi-Agent Orchestrator` subtitle in dim
- **Input field (3 lines):** Accent border, placeholder text "Describe a task or paste an issue URL..." in dim
- **Quick actions card (5 lines):** Rounded border, showing `/issue`, `/monitor`, `/provider` with command in accent and description in dim
- **Recent clusters (variable):** Last 3 clusters from monitor state as clickable rows, or "(no recent clusters)" if empty

**Files:** `ui/launcher.rs`

**Acceptance Criteria**

- Content centered both vertically and horizontally
- Logo and branding visible
- Quick actions provide discoverability for new users
- Existing input functionality unchanged

**Dependencies:** Issue 23.

---

### Issue 26 — Monitor Table Polish

Status: Open

**Goal:** Make the cluster table visually informative at a glance.

**Scope**

- Remove outer block border (let table breathe)
- Header row: dim + bold + underlined
- **Row styling by state:**
  - `running`: normal text, STATE cell green
  - `done`: entire row dimmed, STATE cell in done-green
  - `error`/`failed`: STATE cell red
  - `pending`/`starting`: STATE cell yellow
- Selected row: accent background + dark foreground (lazygit style)
- Highlight symbol: ` > ` (bold)
- **Empty state:** Centered "No active clusters" + hint to use Launcher (Esc)
- Add AGENTS column showing `active/total` format

**Files:** `screens/monitor.rs`

**Acceptance Criteria**

- Cluster state immediately visible via color coding
- Empty state provides clear next action
- Selection is prominent and readable

**Dependencies:** Issue 23.

---

### Issue 27 — Cluster Pane Styling

Status: Open

**Goal:** Improve the 4-pane cluster view with better visual hierarchy.

**Scope**

- **Focus indicator:** Focused pane gets double border (`BorderType::Double`) in accent + bold title wrapped in `[ ]`; unfocused panes get single border in dark gray
- **Layout proportions:** Top row 40/60 (topology narrower, agents wider), bottom 50/50
- **Log coloring:** Agent prefix `[worker]` colored per-agent from rotating palette (hash agent_id to color index)
- **Timeline icons:** Prefix based on topic: `▶` (issue), `●` (implementation), `◆` (validation), `★` (consensus), `·` (default). Label colored by result (green=approved, red=rejected, yellow=pending)
- **Agent list:** Count in title `Agents (3)`, status dot before name, selected agent highlighted with accent bg
- **Metrics:** Move from separate line into header bar (right side)

**Files:** `screens/cluster.rs`, `ui/widgets/topology.rs`

**Acceptance Criteria**

- Focus is immediately visible via border style
- Agent attribution in logs is color-coded
- Timeline events have visual semantic indicators
- No functional/state changes

**Dependencies:** Issue 23.

---

### Issue 28 — Agent Screen + Widget Polish

Status: Open

**Goal:** Final polish pass on agent view and shared widgets.

**Scope**

- **Agent identity:** Status dot with color (`●` green=executing, yellow=waiting, gray=idle, red=error), agent ID and role in assigned agent color
- **Log timestamps:** Prepend `HH:MM:SS` in dim (parse from `ClusterLogLine.timestamp` epoch ms)
- **Guidance input:** Status line above input showing last delivery result (`✓ injected` in green, `✗ failed` in red, `⟳ Sending...` in yellow)
- **Status bar hints:** Per-screen context: Launcher shows `Enter:start`, Monitor shows `j/k:navigate Enter:open`, Cluster shows `Tab:pane j/k:scroll`, Agent shows `Enter:send j/k:scroll`

**Files:** `screens/agent.rs`, `ui/widgets/command_bar.rs`

**Acceptance Criteria**

- Agent status immediately visible via colored dot
- Guidance delivery feedback shown to user
- Keyboard hints adapt to current screen

**Dependencies:** Issues 23–27.
