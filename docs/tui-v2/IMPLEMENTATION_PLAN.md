# Zeroshot TUI v2 (Ink) - Multi-Stage Implementation Plan

Date: 2026-01-25
Status: Draft plan (intended to become a chain of GitHub issues)

## Guiding Constraints

- Ink + TypeScript for all new TUI code.
- Do not refactor unrelated JS during the TUI project.
  - Small, targeted extraction is allowed only when it reduces duplication for TUI integration (e.g. shared `run` helpers).
- Replace the current dashboard feature (`zeroshot watch` / `src/tui/*`) with a new TUI experience launched by `zeroshot` (no args).
- Design/visual polish is explicitly deferred; prioritize UX flows and correctness.

## Proposed Technical Approach (Replace Existing `src/tui`)

### Code organization (proposed)

- `src/tui/` (TypeScript source, Ink components) - replaces the existing blessed dashboard implementation
  - `src/tui/app.tsx` - top-level Ink app, router, global keybindings
  - `src/tui/views/*` - launcher/monitor/cluster/agent views
  - `src/tui/commands/*` - slash command parser + dispatch
  - `src/tui/services/*` - adapters around existing JS orchestrator/ledger
  - `src/tui/domain/*` - typed models (ClusterRow, AgentRow, TimelineEvent, etc)
- `lib/tui/` (compiled JS output shipped in npm package)

Rationale:

- Keeps the existing JS runtime intact.
- Allows incremental TS adoption with minimal surface area.
- Avoids a repo-wide build migration.

### Build strategy (minimal TS emission)

Add a dedicated `tsconfig.tui.json` that:

- includes only `src/tui/**/*.ts(x)`
- emits CJS output to `lib/tui/`
- keeps the existing `tsconfig.json` as "check JS, no emit"

Wire `npm run build:tui` into `prepublishOnly` so `npm pack`/publish contains the compiled Ink app.

### CLI integration strategy

Update `cli/index.js` to:

- open the TUI when `zeroshot` is run with no args in an interactive TTY
- add explicit commands:
  - `zeroshot tui` (always open TUI)
  - `zeroshot codex|claude|gemini|opencode` (open TUI with session provider override)
- keep `zeroshot watch` as an alias (expected to start in Monitor view once implemented).

Important:

- We do not keep the old blessed dashboard in parallel. As soon as the Ink entrypoint exists,
  the old `src/tui/*` implementation is replaced/removed.

## Workstreams

This plan is split into chainable milestones, with parallelizable work inside each milestone.

- Workstream A: CLI + packaging + TypeScript build
- Workstream B: Ink app shell + navigation + command model
- Workstream C: Data adapters (orchestrator, ledger logs, metrics, topology)
- Workstream D: UI views (launcher, monitor, cluster, agent)
- Workstream E: Guidance messaging backend (new capability) + UI wiring
- Workstream F: Tests + reliability hardening

## Milestones and Issues

Below, each "Issue" is intended to become a standalone PR with tight boundaries.

### Milestone 1: Foundations (TS + Ink skeleton)

#### Issue 1.1 - Add Ink + React dependencies (no runtime behavior change)

Scope:

- Add runtime deps needed for Ink TUI (e.g. `ink`, `react`).
- Add dev deps for TS typing/testing (e.g. `@types/react`, `ink-testing-library`), if needed.

Non-scope:

- No CLI behavior changes.
- No new commands.

Acceptance:

- `npm test` still passes.
- Existing CLI commands behave identically.

#### Issue 1.2 - Replace `src/tui/` with "Hello Ink TUI" + build pipeline

Scope:

- Replace the existing blessed dashboard implementation under `src/tui/` with a minimal Ink app
  (start with a single screen that renders and exits cleanly).
- Create `src/tui/index.tsx` (or equivalent entry) that renders a minimal Ink screen.
- Add `tsconfig.tui.json` emitting to `lib/tui/`.
- Add `npm run build:tui` and ensure it runs in `prepublishOnly`.
- Repoint the `zeroshot watch` implementation to the Ink entrypoint (old dashboard is removed immediately).
- Remove/replace any legacy dashboard tests that depend on blessed-contrib layout behavior.

Acceptance:

- `npm run build:tui` produces `lib/tui/index.js`.
- Running a temporary dev entry (e.g. `node -e "require('./lib/tui').start()"`) renders in terminal.
- `npm pack` includes `lib/tui/*`.
- `zeroshot watch` opens the Ink app (even if it is still a minimal stub).

Parallelizable with:

- Issue 1.1

#### Issue 1.3 - Add `zeroshot tui` command (explicit entrypoint) + provider session override

Scope:

- Add `zeroshot tui` command to `cli/index.js` that invokes the compiled Ink app.
- Ensure `--provider <name>` is accepted for `zeroshot tui` (session override only).
- Keep `zeroshot watch` as an alias (expected to start in Monitor view once implemented).

Acceptance:

- `zeroshot tui` opens the Ink app.
- `zeroshot tui --provider codex` passes provider override into the app (visible in UI state).

Depends on:

- Issue 1.2

### Milestone 2: Navigation + Command Model (no cluster execution yet)

#### Issue 2.1 - Implement view router + "Esc back" navigation

Scope:

- Implement 4 view states (even if stubbed):
  - Launcher
  - Monitor
  - Cluster
  - Agent
- Global navigation rule: Esc pops view stack until Launcher.

Acceptance:

- Esc navigation is consistent from every view.
- Ctrl+C exits cleanly (no terminal corruption).

Depends on:

- Issue 1.3

#### Issue 2.2 - Implement command box + slash-command parser (MVP set)

Scope:

- Global input component.
- Parse rules:
  - `/...` => command
  - otherwise => plain-text task description (stubbed for now)
- Implement MVP commands:
  - `/help`, `/monitor`, `/issue <ref>`, `/provider <name>`, `/quit`
- Display command output (toast/status area).

Acceptance:

- Commands work from any view.
- Provider switches update a session state indicator.
- Commands that require orchestration (e.g. `/issue`) can be stubbed here and fully implemented in Milestone 3.

Parallelizable with:

- Issue 2.1

#### Issue 2.3 - Command dispatch scaffolding for "full CLI parity" over time

Scope:

- Introduce a typed command registry that maps `/...` commands to handlers.
- Start with a small compatibility layer so new commands can reuse existing CLI helper functions
  (without requiring users to type `zeroshot ...`).
- Implement 1-2 additional commands as proof (e.g. `/status <id>`, `/list`).

Non-scope:

- Do not implement every command in one PR.
- Avoid large refactors of `cli/index.js`; prefer extracting tiny shared helpers.

Acceptance:

- Adding a new slash command is a small, isolated change (new handler + tests).
- `/status <id>` works end-to-end and renders output in the TUI.

### Milestone 3: Launch Cluster From Text (end-to-end MVP loop)

#### Issue 3.1 - Extract minimal reusable "start cluster" helper for TUI (avoid copying CLI)

Scope:

- Create a small adapter module (prefer `lib/` or `src/` JS) that encapsulates:
  - explicit input construction:
    - plain text (default launcher behavior)
    - issue refs for `/issue ...` (use existing parsing logic, but only for the `/issue` path)
  - provider override resolution
  - loading config (`resolveConfigPath`, `loadClusterConfig`)
  - calling `orchestrator.start(...)`
- TUI calls this helper rather than duplicating CLI logic.

Constraints:

- Keep refactor minimal; do not restructure the CLI wholesale.

Acceptance:

- Existing CLI `zeroshot run` remains unchanged in behavior.
- New helper can be unit-tested independently.

#### Issue 3.2 - Launcher view: Enter launches cluster and transitions to Cluster view

Scope:

- In Launcher view, non-`/` input starts a cluster from plain text with current provider override.
- Ambiguity rule: numeric input like `123` is treated as plain text (never an issue).
- Show cluster id immediately (optimistic UI).
- Transition to Cluster view after start begins.

Acceptance:

- Typing `Implement X` and pressing Enter starts a cluster and switches to Cluster view.
- Failures are shown as a clear error message and user remains in Launcher view.

Depends on:

- Issue 3.1

#### Issue 3.3 - `/issue <ref>` command: run an issue and transition to Cluster view

Scope:

- Implement `/issue <ref>`:
  - `ref` can be: `123`, `org/repo#123`, full issue URL, Jira key, etc (same accepted formats as CLI `zeroshot run <input>`).
  - Start a cluster using that issue ref and current provider override.
  - Transition to Cluster view for that cluster id.

Acceptance:

- `/issue 123` starts a cluster from the issue (no ambiguity with plain text `123`).
- Errors are clearly rendered in the TUI (e.g. missing `gh`, auth failures, invalid ref).

Depends on:

- Issue 3.1

#### Issue 3.4 - Live log streaming in Cluster view (baseline)

Scope:

- Subscribe to the cluster ledger via `messageBus.subscribe(...)` or `ledger.since(...)` polling.
- Render a scrolling log viewport (Ink list/text).
- Provide basic filtering by agent id (optional toggle; can be later).

Acceptance:

- Cluster view shows new log lines within 0.5s of being written.
- No unbounded memory growth (keep only last N lines in view state).

Parallelizable with:

- Issue 3.2 (once cluster id is known)

### Milestone 4: Monitor View (replacement for old dashboard)

#### Issue 4.1 - Monitor view: list clusters from orchestrator registry

Scope:

- Implement `/monitor` view:
  - fetch cluster list from orchestrator
  - display a selectable list (arrow keys + enter)
- Enter opens Cluster view for selected id.

Acceptance:

- Monitor list matches `zeroshot list` cluster table order/contents (within reason).
- Can open any existing cluster (including completed) into Cluster view.

Depends on:

- Milestone 3 (Cluster view exists)

#### Issue 4.2 - Add resource metrics to Monitor view (best effort)

Scope:

- Gather per-agent pid/CPU/memory using existing `pidusage` patterns.
- Aggregate per cluster and show in the list.
- Degrade gracefully on unsupported platforms.

Acceptance:

- On macOS/Linux, CPU/mem fields are populated for running clusters when possible.
- UI remains responsive with 10+ clusters; metrics refresh is throttled (e.g. every 2s).

Parallelizable with:

- Issue 4.1

#### Issue 4.3 - `zeroshot watch` starts directly in Monitor view

Scope:

- Ensure `zeroshot watch` starts the Ink TUI directly in Monitor view (not Launcher).
- Keep `zeroshot tui` defaulting to Launcher view.

Acceptance:

- `zeroshot watch` opens Monitor view as the initial screen.
- No user-facing regression for "monitor clusters" workflow.

Depends on:

- Issue 4.1

### Milestone 5: Cluster Focused View Enhancements (topology, steps)

#### Issue 5.1 - Topology rendering from cluster config

Scope:

- Build a topology model from the running cluster config:
  - agents (id, role)
  - triggers/edges (topic wiring)
- Render as:
  - MVP: adjacency list / ASCII tree / simple box diagram
  - keep layout engine out-of-scope for now

Acceptance:

- For built-in templates, topology view shows all agents and their relationships.
- Works for dynamically added agents (best effort; show as appended nodes).

#### Issue 5.2 - Step timeline derived from workflow triggers (MVP)

Scope:

- Define a minimal "timeline event" schema for the TUI.
- Populate it from `WORKFLOW_TRIGGERS` messages (PLAN_READY, IMPLEMENTATION_READY, VALIDATION_RESULT, etc).
- Render a compact history list in Cluster view.

Acceptance:

- Timeline shows the major phases for a typical run.
- Timeline persists on resume (derived from ledger, not in-memory only).

Parallelizable with:

- Issue 5.1

Pin:

- Later we can enrich the timeline with additional lifecycle events/state transitions for better fidelity.

### Milestone 6: Agent View (drill-down + messaging UI)

#### Issue 6.1 - Agent selection + Agent view log tail

Scope:

- In Cluster view, allow selecting an agent (arrow keys) and opening Agent view (Enter).
- Agent view tails logs for that agent only.

Acceptance:

- Agent view shows live agent logs and updates in near real time.
- Esc returns to Cluster view preserving selection.

Depends on:

- Issue 3.4

#### Issue 6.2 - Agent messaging UI (stubbed backend)

Scope:

- Add an input box in Agent view for sending messages.
- For now, messages can be recorded as "pending" without delivery (backend not yet wired).

Acceptance:

- User can type and submit a message; UI shows it as queued.
- No crashes if backend is missing.

Parallelizable with:

- Milestone 7 backend work

### Milestone 7: Guidance Messaging (new backend capability)

This milestone is required to meet the PRD's "steer agents/cluster live" vision. It is intentionally separated because it touches core orchestration behavior.

#### Issue 7.1 - Ledger topic + mailbox schema for guidance

Scope:

- Define new message topics (example):
  - `USER_GUIDANCE_CLUSTER`
  - `USER_GUIDANCE_AGENT`
- Each message includes:
  - `cluster_id`, `sender: "user"`, `topic`, `content.text`
  - optional `target_agent_id`
  - delivery state fields (optional, can be derived)
- Implement a mailbox query helper for "guidance since last delivered".

Acceptance:

- Guidance messages are persisted in the ledger.
- Queries for "undelivered guidance" are deterministic and testable.

#### Issue 7.2 - Live injection plumbing (provider stdin/PTY) with graceful detection

Scope:

- Implement a provider-agnostic "send input" path that writes directly to the underlying provider CLI session stdin/PTY when available.
  - This should reuse the same mechanism the provider uses for interactive input (e.g. node-pty stdin write).
- Persist the guidance message in the ledger regardless (for audit/history), but mark whether it was injected live.
- If a given agent/provider session does not expose an interactive stdin handle, return "unsupported" so callers can fallback to queue semantics (Issue 7.3).

Acceptance:

- For at least one provider with interactive sessions, sending guidance while the agent is working injects into the live session.
- If injection is not possible, the API signals that it was not injected (so UI can show "queued").

Depends on:

- Issue 7.1

#### Issue 7.3 - Queue fallback: apply guidance at safe points

Scope:

- Implement a safe-point mailbox consumer in the agent execution loop:
  - fetch queued guidance for (cluster, agent)
  - append to the next provider prompt in a controlled way (clearly delimited)
- Ensure guidance does not break JSON-schema modes or structured output.

Acceptance:

- In a test cluster for a non-injectable mode/provider, guidance sent mid-run appears in the next agent prompt.
- No regressions for existing runs (no guidance => behavior unchanged).

Depends on:

- Issue 7.2

#### Issue 7.4 - Cluster-wide guidance broadcast (injection-first, fallback-queue)

Scope:

- Implement cluster-level guidance delivery:
  - write a single cluster-scoped message
  - each agent attempts live injection when possible (Issue 7.2)
  - otherwise the message is applied at that agent's next safe point (Issue 7.3)

Acceptance:

- Cluster-level guidance reaches all agents:
  - injected live when supported
  - queued otherwise

Depends on:

- Issue 7.3

#### Issue 7.5 - Wire TUI messaging UI to backend guidance mechanism

Scope:

- Cluster view: guidance box sends `USER_GUIDANCE_CLUSTER`.
- Agent view: guidance box sends `USER_GUIDANCE_AGENT`.
- Display delivery feedback (injected/queued/applied if detectable).

Acceptance:

- Messages typed in the TUI are persisted and delivered to agents (per Issues 7.1-7.4).

Depends on:

- Issues 6.2, 7.4

### Milestone 8: Default Entry + Cleanup + Hardening

#### Issue 8.1 - `zeroshot` (no args) launches TUI (TTY only)

Scope:

- Modify `cli/index.js` default behavior:
  - if interactive TTY and no subcommand: open TUI launcher
  - else: keep existing help output behavior

Acceptance:

- `zeroshot` opens TUI in a normal terminal.
- `echo foo | zeroshot` does not hang (prints help and exits).

Depends on:

- Milestone 3 (minimum viable launcher exists)

#### Issue 8.2 - Add `zeroshot codex|claude|gemini|opencode` convenience entrypoints

Scope:

- Add CLI commands that invoke `zeroshot tui --provider <name>`.

Acceptance:

- `zeroshot codex` opens TUI with codex selected for the session.

Depends on:

- Issue 8.1 (or earlier if `zeroshot tui --provider` is already done)

#### Issue 8.3 - Cleanup: remove legacy dashboard docs/tests and stale references

Scope:

- Remove any remaining blessed-dashboard-specific docs, demos, and tests (after the Ink TUI replacement).
- Ensure CLI help text and docs point to the Ink TUI entrypoints (`zeroshot`, `zeroshot tui`, `zeroshot watch`).

Acceptance:

- No references remain to the old blessed dashboard behavior.

Depends on:

- Issue 1.2

#### Issue 8.4 - Reliability/Perf pass + tests

Scope:

- Add tests:
  - slash command parsing
  - router "Esc back" behavior
  - monitor list rendering
  - guidance mailbox behavior (if Milestone 7 done)
- Manual checklist:
  - resize behavior
  - exit behavior (terminal reset)
  - running multiple clusters
  - resume existing cluster into Cluster view

Acceptance:

- CI is green.
- No known terminal corruption issues on exit.

## Parallelization Map (Suggested)

- Team 1:
  - Milestone 1 (deps/build) + Milestone 2 (router/commands)
- Team 2:
  - Milestone 3 (cluster start + logs) once helper exists
- Team 3:
  - Milestone 4 (monitor view + metrics)
- Team 4:
  - Milestone 7 (guidance backend) can start as soon as requirements are agreed
- Team 5:
  - Milestone 5 (topology + timeline) can start once cluster config/state access is finalized

## Definition of Done (Project-level)

The project is considered "v2 shipped" when:

- `zeroshot` launches the Ink TUI (TTY only) and can start a cluster from text.
- `/monitor` provides a stable cluster dashboard and drill-down.
- Cluster view supports logs + topology + timeline.
- The blessed-based dashboard is removed; `zeroshot watch` is an alias that opens the Ink TUI Monitor view.

## Deferred / Post-v2 Candidates

- AI summary panel (`/summary` or periodic): provider/model choice and cost controls TBD.
- Enrich the step timeline with additional lifecycle events beyond `WORKFLOW_TRIGGERS`.
