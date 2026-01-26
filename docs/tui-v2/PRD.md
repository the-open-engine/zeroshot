# Zeroshot TUI v2 (Ink) - PRD

Date: 2026-01-25
Owner: Zeroshot CLI
Status: Draft

## Summary

Build a new terminal UI (TUI) for Zeroshot using Ink + TypeScript. This fully replaces the current `zeroshot watch` dashboard (blessed-based) with a single interactive experience launched by running `zeroshot` (no args).

Core workflow:

- Open TUI
- Type a task description into a central input box
- Press Enter to launch a cluster
- Immediately switch to the focused cluster view (topology + logs + progress)
- Use `/monitor` to see a high-level dashboard of all clusters, then drill down via Enter
- Navigate back with Esc (Esc always steps back until the launcher view)

Provider selection is session-scoped and can be chosen at launch (`zeroshot codex|claude|gemini|opencode`) or switched in-TUI.

## Goals

1. Replace the current dashboard feature with an Ink-based TUI.
2. Make `zeroshot` (no args) a first-class interactive mode for:
   - launching clusters from free-form text
   - monitoring all running clusters
   - drilling into clusters and individual agents
3. Add a command palette/input model:
   - plain text launches a cluster
   - `/`-prefixed commands run zeroshot operations without re-typing `zeroshot`
4. Establish the first production TypeScript surface in the Zeroshot codebase (TUI + required adapters), without refactoring unrelated JS.

## Non-Goals (for v2 MVP)

- No pixel-perfect or final visual design (layout/theme/typography can evolve).
- No full JavaScript -> TypeScript migration.
- No remote multi-user UI; TUI is local-only.
- No promise of message delivery for providers/modes that do not support interactive stdin injection.
  - In those cases, guidance is queued and applied at the next safe point.
- No replacement of non-dashboard CLI commands; existing subcommands remain supported.

## Users / Personas

- Power users running many clusters and needing quick navigation (htop/k9s style).
- Developers launching a cluster from an idea ("just type it and go").
- Operators who need to see whether a run is stuck, progressing, or consuming resources.

## Glossary

- Cluster: a multi-agent run managed by `src/orchestrator.js`, persisted in `~/.zeroshot/*.db`.
- Agent / Worker: an `AgentWrapper` instance inside a cluster (worker, planner, validators, etc).
- Provider: external CLI used for agent reasoning (claude, codex, gemini, opencode).

## Entry Points

### `zeroshot` (no args)

Expected behavior:

- If running in an interactive TTY: start TUI in Launcher view.
- If not a TTY (piped/CI): print help and exit non-zero only on misuse (avoid breaking scripts).

### `zeroshot tui` (explicit)

Always opens the TUI (even if additional flags are provided).

### `zeroshot codex|claude|gemini|opencode`

Opens the TUI with a session-scoped provider override. While the TUI is open:

- any cluster launch uses the chosen provider (equivalent to passing `--provider <name>` behind the scenes)
- switching provider in-TUI updates this override for subsequent operations

Notes:

- This must not permanently change the default provider setting. Persisted defaults remain managed by `zeroshot providers set-default ...`.

### `zeroshot watch`

`zeroshot watch` remains as a convenience alias that opens the new Ink TUI directly in Monitor view.

The existing blessed-based TUI implementation is removed (no parallel legacy dashboard).

## Core Navigation / Views

### 1) Launcher View (Main)

Primary UI shown on start.

Content:

- A central input box for text.
- Short instructions/hints (e.g. `/monitor`, `/help`, provider indicator).

Input semantics:

- If input starts with `/`: treat as a command (see Commands).
- Else: treat as plain-text task description and start a cluster.
  - Ambiguity rule: numeric input like `123` is treated as plain text.
  - To run an issue, use `/issue ...` (e.g. `/issue 123`).

On Enter with non-command input:

- Start cluster
- Transition to Cluster Focused View for that cluster

### 2) Monitor View (Dashboard)

Opened by `/monitor` from anywhere.

Displays a high-level list of clusters, including:

- cluster id
- input/task name (derived from input: issue title, file name, or first line of text)
- provider used
- status (running/completed/failed/stopped/corrupted)
- running time (duration)
- agents/workers count
- resource usage (CPU%, memory) aggregated across agents (best effort; degrade gracefully)
- token usage (if available via ledger aggregation)
- last activity timestamp

Interaction:

- Up/Down (or j/k) moves selection
- Enter opens Cluster Focused View for selected cluster
- Esc goes back to previous view

Optional (later, but supported by design):

- filtering (running/stopped/all)
- search by cluster id / task substring
- actions: stop/kill/export via `/stop <id>`, `/kill <id>`, `/export <id>`

### 3) Cluster Focused View

Opened automatically after launching a cluster, or by selecting a cluster from Monitor view.

Must display:

1. Cluster topology graph:
   - Rendered from the cluster config template (agents + triggers).
   - MVP: tree/adjacency list/ASCII graph; fidelity can improve over time.
2. Live logs:
   - Streamed from the ledger message bus (append-only).
   - Includes per-agent attribution (role/agent id).
3. Step timeline / phase log:
   - High-level step list derived from workflow triggers (`WORKFLOW_TRIGGERS`) for MVP.
   - Examples: plan -> implement -> validate -> iterate -> complete.
   - Exact naming/format is defined by the TUI domain model and can evolve.

Interaction:

- Left/Right (or Tab) cycles focus between panes (topology/logs/steps/agents list).
- Up/Down moves selection when in a selectable pane (e.g. agents list).
- Enter on a selected agent opens Agent View.
- Esc goes back (Monitor if you came from Monitor, or Launcher if you came from Launcher).

Cluster guidance input:

- A command/text box is available in this view to send guidance to the whole cluster.
- This requires backend support (see "Guidance Messaging").

### 4) Agent View (Focused Worker/Agent)

Opened by selecting an agent from Cluster Focused View.

Must display:

- Live logs for that agent (tailing relevant ledger output)
- Agent identity (role, provider, model level/model)
- Agent status (idle/executing/stuck/failed/completed)

Agent messaging input:

- A command/text box that lets the user send guidance to that agent while it is working.
- Provider capability may vary. If true "live injection" is not available, guidance is queued and applied at the next safe point.

Esc returns to Cluster Focused View.

## Commands (Slash Commands)

General:

- Any view can accept `/...` commands.
- Commands should be parsed similarly to CLI subcommands, but do not require the `zeroshot` prefix.
- Commands produce feedback in a lightweight output/toast area (success/failure).

Parity target:

- Long-term: every existing `zeroshot <cmd ...>` operation should be invocable as `/<cmd ...>` inside the TUI.
- Short-term: ship an MVP subset first (below), then expand to full parity incrementally.

Required commands for MVP:

- `/help` - show available commands and keybindings
- `/monitor` - open Monitor view
- `/issue <ref>` - start a cluster from an issue reference (e.g. `123`, `org/repo#123`, URL, Jira key, etc)
- `/provider <name>` - switch provider for the session (claude|codex|gemini|opencode)
- `/quit` (and `/exit`) - exit the TUI (with confirmation if clusters running)

Strongly recommended commands (v2 follow-up):

- `/run <input>` - start a cluster using CLI-style input parsing (issue/file/text auto-detection)
- `/list` - list clusters/tasks (or open Monitor view)
- `/status <id>` - show detailed cluster/task status
- `/logs <id>` - open Cluster Focused View (or open logs modal)
- `/stop <id>` and `/kill <id>` - control cluster/task

## Guidance Messaging (Backend Requirement)

Two scopes:

1. Cluster guidance: message broadcast to all agents in the cluster.
2. Agent guidance: message targeted to a specific agent.

Delivery semantics (MVP):

- Attempt live stdin injection into the underlying provider CLI session when supported.
  - Implementation detail: write to the provider process/PTY stdin (same mechanism the provider uses for interactive chat).
- If live injection is not supported (provider limitation or non-interactive mode), queue the guidance:
  - store in the ledger (or a small mailbox store)
  - agents apply it at the next safe point:
    - before starting a new iteration
    - before generating a new provider prompt
    - after finishing a tool execution step

TUI requirements:

- Show whether a guidance message was injected live or queued.
- Indicate delivery status (pending/applied/expired) when possible.

## Data Sources / Domain Model

The TUI reads from:

- Orchestrator registry: cluster list and state (`src/orchestrator.js`)
- Ledger message bus: logs, events, tokens (`src/message-bus.js`, `src/ledger.js`)
- Process metrics: pid/CPU/mem per agent (existing `pidusage` usage)
- Cluster config: topology information (from resolved config JSON)

The TUI writes:

- Start/stop/kill commands via orchestrator
- Guidance messages via message bus / mailbox

## Non-Functional Requirements

Performance:

- TUI launch time (from `zeroshot` to first paint): < 500ms on a typical dev machine.
- UI update cadence:
  - logs: event-driven, or up to 200-500ms batching
  - cluster list refresh: 1s (configurable)
  - resource metrics refresh: 2s (configurable, because pidusage can be expensive)

Reliability:

- No terminal corruption on exit (restore cursor, clear alternate buffer if used).
- Handles terminal resize gracefully.
- Works on macOS and Linux. If metrics are unsupported, show "-" and keep UI functional.

Security/Privacy:

- Do not exfiltrate code or logs; all data stays local.
- Avoid rendering secrets in the UI where possible (best-effort; user controls what runs).

## Success Metrics

Quantitative:

- 90%+ of interactive "start cluster from text" actions succeed without leaving the TUI.
- 0 known cases of terminal left in broken state on normal exit.
- Monitor view can handle 100 clusters in the registry without freezing (best-effort; virtualization if needed later).

Qualitative:

- Users can launch, monitor, and drill down without remembering command syntax.
- Navigation feels consistent (Esc always goes back).

## Out of Scope / Deferred

- Full parity with all CLI flags (`--docker`, `--ship`, `--worktree`, etc) in the launcher input.
  - These can be added incrementally via `/run --docker ...` style.
- Rich graphs (true layout engine). MVP uses simplified ASCII representation.
- Advanced UX (themes, mouse, split panes, persistent layouts).
- AI summary panel (provider/model choice + cost controls TBD).

## Resolved Decisions (from product feedback)

1. Ambiguous launcher input:
   - Launcher treats non-`/` input as plain text (including `123`).
   - Use `/issue 123` (or another issue ref) to run an issue.
2. Step timeline schema:
   - MVP derives timeline from existing workflow triggers (`WORKFLOW_TRIGGERS`).
   - We keep a pin to later enrich with additional lifecycle events.
3. AI summary:
   - Deferred (pin). Implement after core TUI flows ship.
4. Guidance injection:
   - Implement live injection when provider supports it (inject into the provider CLI stdin/PTY).
   - Otherwise, queue and apply at safe points.
