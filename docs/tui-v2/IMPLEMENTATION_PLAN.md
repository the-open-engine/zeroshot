# Zeroshot TUI v2 (Ratatui) - Multi-Stage Implementation Plan

Date: 2026-01-30 (updated; original PRD date: 2026-01-25)
Status: Draft plan (intended to become a chain of GitHub issues)
Source of truth: `docs/tui-v2/PRD.md`

## Guiding Constraints

- **Ratatui frontend** (Rust) for fast rendering. Ink/React UI is no longer the target frontend.
- **Detached clusters by default**: clusters launched from the TUI must keep running after the TUI exits.
- **Strict layering**: UI owns layout/input only; orchestration/domain logic lives in a headless backend.
- **No ad-hoc refactors** of Zeroshot core runtime: small, targeted extraction is allowed only when it
  reduces duplication (e.g., a shared detached launcher used by both CLI + TUI backend).
- **No ports**: frontend <-> backend communication is over stdio using a small typed protocol.
- **Performance budgets** (from PRD):
  - < 500ms first paint
  - logs batching: 200-500ms
  - cluster list refresh: 1s
  - metrics refresh: 2s
- **Reliability**:
  - always restore terminal state on exit/crash
  - handle resize
  - bounded memory (ring buffers for logs/events)

## Architecture (Final Target)

Two-process design:

```
┌──────────────────────────┐          NDJSON over stdio         ┌──────────────────────────┐
│ zeroshot-tui (Rust)       │  <──────────────────────────────>  │ tui-backend (Node/TS)     │
│ - ratatui renderer        │                                     │ - orchestrator adapters   │
│ - input/keymaps           │                                     │ - ledger readers/streams  │
│ - view stack + layout     │                                     │ - start/stop/kill/guidance│
└──────────────────────────┘                                     └──────────────────────────┘
```

Key properties:

- The Rust process is the **only** thing that touches terminal rendering.
- The TS backend is a thin, testable adapter around existing Zeroshot JS runtime:
  - reads: clusters registry, ledger DB, metrics
  - writes: start/stop/kill commands, guidance send + delivery semantics
- The backend must support **server-push** events for log/timeline streams.

## Code Organization (Target)

- Backend:
  - `src/tui-backend/` (TypeScript source)
  - `lib/tui-backend/` (compiled output shipped)
- Frontend:
  - `tui-rs/` (Cargo workspace)
  - `tui-rs/crates/zeroshot-tui/` (ratatui binary)
- CLI integration:
  - `lib/tui-rs/launcher.js` (Node helper that launches the correct binary and wires env vars)

## Packaging Strategy (Recommended)

Goal: `npm i -g @covibes/zeroshot` must include a working Ratatui TUI without requiring Rust toolchains.

Recommended approach (esbuild-style):

- Publish platform-specific optional dependency packages containing the `zeroshot-tui` binary:
  - `@covibes/zeroshot-tui-darwin-arm64`
  - `@covibes/zeroshot-tui-darwin-x64`
  - `@covibes/zeroshot-tui-linux-x64-gnu`
  - (more as needed)
- The main `@covibes/zeroshot` package declares these as `optionalDependencies`.
- `lib/tui-rs/launcher.js` resolves the installed platform package and spawns the binary with `stdio: inherit`.

This keeps installs fast, avoids postinstall compilation, and makes failures explicit (missing prebuild).

MVP alternative (allowed for early dev iteration only):

- build from source locally and point `ZEROSHOT_TUI_BIN` at `target/debug/zeroshot-tui`.

## Milestones and Issues

Each issue below is intended to be a standalone PR with tight boundaries and clear acceptance steps.

### Milestone 0: Protocol + Dual-Process Skeleton (No Real UI Yet)

#### Issue TUI-0001: Define protocol v1 (requests/responses/events) and NDJSON framing

Scope:

- Define the wire envelopes (1 JSON object per line):
  - request: `{ "type": "req", "id": "uuid", "method": "clusters.list", "params": { ... } }`
  - response: `{ "type": "res", "id": "uuid", "ok": true, "result": { ... } }`
  - error: `{ "type": "res", "id": "uuid", "ok": false, "error": { "code": "...", "message": "...", "data": { ... } } }`
  - event: `{ "type": "evt", "event": "logs.lines", "params": { ... } }`
- Add canonical TS types:
  - `src/tui-backend/protocol/v1.ts`
- Add mirrored Rust structs:
  - `tui-rs/crates/zeroshot-tui/src/backend/protocol.rs`
- Add a short protocol README with examples and invariants:
  - ordering, backpressure expectations, and version handshake behavior.

Non-scope:

- No orchestration logic yet.

Acceptance:

- TS and Rust compile.
- Roundtrip framing tests exist on both sides (req/res/evt).

#### Issue TUI-0002: Scaffold headless TS backend process with `system.hello` + `system.ping`

Scope:

- Add `src/tui-backend/main.ts`:
  - read stdin lines
  - parse protocol requests
  - respond to:
    - `system.hello` (protocol version + backend build info)
    - `system.ping` (returns `{ nowMs }`)
- Add `tsconfig.tui-backend.json` emitting to `lib/tui-backend/`.
- Add `npm run build:tui-backend` and wire into `prepublishOnly`.

Acceptance:

- `node lib/tui-backend/main.js` responds to a piped `system.ping`.
- Backend exits cleanly on stdin EOF.

#### Issue TUI-0003: Scaffold Rust ratatui app that spawns backend and performs `system.hello`

Scope:

- Add Cargo workspace at `tui-rs/`.
- Create `tui-rs/crates/zeroshot-tui` binary:
  - `ratatui` + `crossterm` backend
- Implement:
  - terminal init/restore (raw mode + alternate screen)
  - render a simple “Connected” screen
  - spawn backend child process and issue `system.hello`
- Use env vars:
  - `ZEROSHOT_NODE_EXEC_PATH` (node executable path)
  - `ZEROSHOT_TUI_BACKEND_ENTRY` (absolute path to `lib/tui-backend/main.js`)

Acceptance:

- `cargo run -p zeroshot-tui` shows backend version info and exits cleanly on Ctrl+C.

### Milestone 1: Backend Domain APIs (Monitor + Cluster Data)

#### Issue TUI-0101: Re-home existing TS domain services out of Ink UI

Goal: salvage existing TS domain work by making it frontend-agnostic.

Scope:

- Move (or re-export) these modules into `src/tui-backend/services/`:
  - cluster registry + metrics (from `src/tui/services/cluster-registry.ts`)
  - log streaming (from `src/tui/services/cluster-logs.ts`)
  - timeline streaming (from `src/tui/services/cluster-timeline.ts`)
  - topology model builder (from `src/tui/services/cluster-topology.ts`)
  - guidance delivery (from `src/tui/services/guidance-delivery.ts`)
  - cluster launcher (from `src/tui/services/cluster-launcher.ts`)
- Ensure `src/tui-backend/**` has **zero** Ink/React imports.

Acceptance:

- Backend build includes these services and they can be imported from `lib/tui-backend/...`.

#### Issue TUI-0102: Implement `clusters.list` with PRD-required fields

Scope:

- Backend method: `clusters.list -> { clusters: ClusterRow[] }` (best effort fields)
  - `id`, `state`, `createdAt`, `durationMs`
  - `displayName` derived from `ISSUE_OPENED` (title first, else first line)
  - `provider` derived from cluster config (`forceProvider || defaultProvider`)
  - `agentCount`, `messageCount`
  - `lastActivityAt` from last ledger message timestamp
  - `metrics`: cpu%, memMB (null if unsupported)
  - `tokens`: aggregated totals if available (null otherwise)
- Add backend-side refresh throttling:
  - list refresh: 1s default
  - metrics refresh: 2s default

Acceptance:

- A small manual test script can print `clusters.list` output and it includes stable `displayName`.

#### Issue TUI-0103: Implement `cluster.get` and `cluster.agents`

Scope:

- Backend method: `cluster.get { clusterId }`
  - cluster summary + cwd/worktree/isolation info (best effort)
- Backend method: `cluster.agents { clusterId }`
  - `id`, `role`, `state`, `iteration`, `currentTaskId`, `processPid`
  - provider/model display fields (best effort derived from config + defaults)

Acceptance:

- For a running cluster, `cluster.agents` returns at least 1 agent and has stable IDs/roles.

### Milestone 2: Backend Streaming APIs (Logs + Timeline + Topology)

#### Issue TUI-0201: Implement `logs.subscribe` (cluster + optional agent filter)

Scope:

- Backend method: `logs.subscribe { clusterId, agentId?: string|null } -> { subscriptionId }`
  - Emits `logs.status` events (idle/waiting/ready/error)
  - Emits `logs.lines` events with batched lines (<= 250ms cadence)
  - Supports unsubscribe: `logs.unsubscribe { subscriptionId }`
- Enforce bounded buffering (ring) in backend.

Acceptance:

- For a running cluster, logs stream continuously; process memory stays bounded under load.

#### Issue TUI-0202: Implement `timeline.subscribe` (workflow triggers only)

Scope:

- Backend method: `timeline.subscribe { clusterId } -> { subscriptionId }`
  - Emits `timeline.status`
  - Emits `timeline.events` (workflow triggers only)
  - Supports unsubscribe

Acceptance:

- Timeline events appear for a cluster containing `PLAN_READY`, `IMPLEMENTATION_READY`, etc.

#### Issue TUI-0203: Implement `topology.get` returning a stable topology model

Scope:

- Backend method: `topology.get { clusterId }`
  - `agents[] { id, role }`
  - `edges[] { from, to, topic, kind, dynamic? }`
  - `topics[]`

Acceptance:

- For standard templates, topology is non-empty and stable across calls.

### Milestone 3: Detached Cluster Launching (Critical Requirement)

#### Issue TUI-0301: Extract reusable detached cluster launcher (shared by CLI + TUI backend)

Scope:

- Create a new internal module under `lib/` that exposes:
  - `spawnDetachedCluster({ input, options, providerOverride, cwd, configName, modelOverride, ... }) -> { clusterId }`
- Refactor existing CLI `run --detach` path to call this helper (no behavior change).
- Preserve current daemon env semantics (`ZEROSHOT_DAEMON=1`, `ZEROSHOT_CLUSTER_ID`, etc).

Acceptance:

- `zeroshot run <text> --detach` works exactly as before.
- Helper can be invoked from a Node script to spawn a detached cluster.

#### Issue TUI-0302: Implement backend `clusters.start` using detached launcher

Scope:

- Backend method: `clusters.start { input: { kind: "text"|"issue"|"file", value: string }, providerOverride?: string|null }`
  - Always launches detached
  - Returns `{ clusterId }` immediately
- Backend emits `clusters.changed` when the new cluster appears in the registry.

Acceptance:

- Launching from the Rust TUI keeps running after the TUI exits.

### Milestone 4: Rust App Core (State + Input + Backend Client)

#### Issue TUI-0401: Rust app state model + reducer (testable)

Scope:

- Implement reducer-style architecture:
  - `Action` (user input, backend events, timers)
  - `State` (view stack, selections, input buffer, focus pane)
  - `reduce(state, action) -> state`
- Unit tests for invariants:
  - Esc pops view stack (stops at launcher)
  - Tab cycles focus
  - j/k move selection within bounds

Acceptance:

- `cargo test -p zeroshot-tui` includes reducer tests and passes.

#### Issue TUI-0402: Rust backend client over stdio (req/res + event stream)

Scope:

- Request correlation by `id`
- Event dispatch by `event` name
- Graceful shutdown (kill backend child process on exit)

Acceptance:

- Rust app can call `system.ping` and display roundtrip latency.

#### Issue TUI-0403: Centralize theme + layout modules for fast iteration

Scope:

- Add:
  - `ui/theme.rs` (colors, styles, spacing constants)
  - `ui/layout.rs` (pane splits per view)
- Ensure view rendering reads from these modules (no scattered constants).

Acceptance:

- Adjusting a layout constant in one file changes the UI without touching domain logic.

### Milestone 5: Views MVP (Launcher + Monitor + Cluster + Agent)

#### Issue TUI-0501: Monitor view MVP (cluster list)

Scope:

- Implement Monitor view:
  - poll `clusters.list` every 1s (or consume `clusters.changed`)
  - render selectable list with columns:
    - id, displayName, state, duration, provider, cpu, mem, lastActivity
  - Enter opens Cluster view for selected cluster
  - Esc goes back

Acceptance:

- With multiple clusters, list is responsive and selection works.

#### Issue TUI-0502: Launcher view MVP (input + hints + start)

Scope:

- Implement Launcher view:
  - central input box
  - hints: `/monitor`, `/help`, provider indicator
  - Enter:
    - `/...` => command dispatch
    - else => call `clusters.start` and navigate to cluster view for returned `clusterId`

Acceptance:

- Entering text launches a detached cluster and transitions to Cluster view.

#### Issue TUI-0503: Cluster view MVP (pane layout + agents list)

Scope:

- Implement Cluster view panes (placeholders acceptable initially):
  - Topology
  - Logs
  - Timeline
  - Agents list (real data from `cluster.agents`)
- Focus cycling (Tab / Left/Right).
- Enter on selected agent opens Agent view.

Acceptance:

- Agents list renders and selection/open works.

#### Issue TUI-0504: Agent view MVP (filtered logs + metadata)

Scope:

- Implement Agent view:
  - identity + state header
  - agent-filtered logs stream (`logs.subscribe` with `agentId`)
  - input box stubbed (message vs command mode later)

Acceptance:

- Agent view shows primarily that agent's output (best effort filter).

### Milestone 6: Streaming UI (Logs + Timeline + Topology Rendering)

#### Issue TUI-0601: Log viewer widget (scroll + follow mode + bounded buffer)

Scope:

- Reusable log viewer widget:
  - ring buffer (max lines)
  - follow-tail toggle
  - scroll controls (PgUp/PgDn or Ctrl+u/Ctrl+d)
- Wire to backend `logs.subscribe`.

Acceptance:

- Logs update smoothly without flicker and memory stays bounded.

#### Issue TUI-0602: Timeline widget wired to workflow triggers

Scope:

- Wire to backend `timeline.subscribe`.
- Render ordered step list with status coloring:
  - pending/in-progress/success/failure (best effort based on topics + approved flag)

Acceptance:

- Workflow events appear and update as cluster progresses.

#### Issue TUI-0603: Topology renderer wired to backend topology model

Scope:

- Render ASCII topology from `topology.get`:
  - MVP: adjacency list or simple graph
- Keep renderer isolated so swapping strategies is a small change.

Acceptance:

- Topology pane shows meaningful structure for standard templates.

### Milestone 7: Commands + Provider Overrides + Stop/Kill

#### Issue TUI-0701: Slash command system in Rust (MVP)

Scope:

- Implement commands:
  - `/help`
  - `/monitor`
  - `/provider <name>`
  - `/issue <ref>`
  - `/quit` and `/exit`
- Show output in a status/toast area.

Acceptance:

- Commands work from any view and provider indicator updates immediately.

#### Issue TUI-0702: Stop/kill from TUI (with confirmation UX)

Scope:

- Implement:
  - `/stop <clusterId>`
  - `/kill <clusterId>`
- Minimal confirmation UX for destructive ops.

Acceptance:

- Stopping/killing updates Monitor view within 1s.

### Milestone 8: Guidance Messaging (Cluster + Agent)

#### Issue TUI-0801: Cluster guidance input + delivery summary UI

Scope:

- In Cluster view, allow plain text (non-`/`) to send cluster guidance:
  - backend method: `guidance.clusterSend { clusterId, text }`
  - show injected vs queued counts
  - keep small history (last N) visible

Acceptance:

- Guidance send works and shows injected/queued feedback.

#### Issue TUI-0802: Agent guidance input + pending message state

Scope:

- In Agent view, allow typing guidance:
  - backend method: `guidance.agentSend { clusterId, agentId, text }`
  - show per-message delivery status (pending -> injected/queued/error)

Acceptance:

- Sending guidance updates delivery status UI.

### Milestone 9: CLI Integration + Packaging (Make `zeroshot` Launch Ratatui)

#### Issue TUI-0901: Add Node launcher for Ratatui binary (dev path first)

Scope:

- Add `lib/tui-rs/launcher.js`:
  - resolve binary path (env override first: `ZEROSHOT_TUI_BIN`)
  - spawn with `stdio: inherit`
  - set env vars:
    - `ZEROSHOT_NODE_EXEC_PATH` (from `process.execPath`)
    - `ZEROSHOT_TUI_BACKEND_ENTRY` (absolute path to `lib/tui-backend/main.js`)
    - `ZEROSHOT_TUI_INITIAL_VIEW` (`launcher|monitor`)
    - `ZEROSHOT_TUI_PROVIDER_OVERRIDE` (optional)

Acceptance:

- Running the launcher spawns the Rust TUI and connects to backend successfully.

#### Issue TUI-0902: Update CLI entrypoints to spawn Ratatui TUI (`zeroshot`, `zeroshot tui`, `zeroshot watch`)

Scope:

- Update `cli/index.js`:
  - `zeroshot` (no args, TTY) opens Ratatui TUI (launcher view)
  - `zeroshot tui` opens Ratatui TUI
  - `zeroshot watch` opens Ratatui TUI in monitor view
- Clear error message if binary missing + remediation.

Acceptance:

- `zeroshot` opens Ratatui UI; `zeroshot watch` starts in Monitor view.

#### Issue TUI-0903: Ship prebuilt binaries via optional dependency packages (release hardening)

Scope:

- Introduce platform binary packages (esbuild-style).
- Update release workflows to build and publish binaries per platform.
- Update launcher to resolve installed platform package.

Acceptance:

- Fresh global install on macOS/Linux has a working TUI without Rust toolchains.

### Milestone 10: Hardening + UX Polish

#### Issue TUI-1001: Resize handling + redraw correctness

Scope:

- Handle resize events; preserve selection/scroll.

Acceptance:

- Resizing terminal does not crash or corrupt layout.

#### Issue TUI-1002: Backpressure + perf budget visibility

Scope:

- Enforce bounds:
  - max buffered log lines per view
  - max in-flight requests
- Add optional debug overlay (toggle) showing render ms and event queue depth.

Acceptance:

- Under high log throughput, UI remains responsive and memory stays bounded.

#### Issue TUI-1003: Cleanup legacy Ink UI (after Ratatui parity)

Scope:

- Remove Ink-specific TUI entrypoints and deps once Ratatui is production-ready.
- Ensure docs reference Ratatui as the supported TUI.

Acceptance:

- Repo no longer relies on Ink for TUI functionality.
