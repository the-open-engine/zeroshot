# Zeroshot Disruptive TUI — Pre-M3 Decisions

Date: 2026-02-01
Status: Accepted (M3 baseline)

## Context

M3 (Live Cluster Canvas v1) needs stable interaction decisions to avoid reworking navigation and rendering. These choices set the baseline for M3 implementation and can be revised post-M3.

## Decisions

### 1) Focus model

**Decision:** Explicit focus ring navigation.

- Focus moves only via explicit keys (Tab/Shift-Tab, Left/Right where applicable).
- The focused pane is visually distinct (border + title treatment); unfocused panes do not capture navigation inputs.
- No automatic “nearest” focus selection based on cursor position or layout changes.

**Rationale:** Predictable keyboard navigation across resizes/layout tweaks; reduces accidental focus shifts and layout-driven churn.

**Revisit if:** We add pointer/hover interactions or multi-focus gestures that justify spatial focus heuristics.

### 2) Label strategy

**Decision:** Always show stable identifiers; no hover/tooltip dependence.

- Primary labels show stable IDs (cluster id, agent id/role) everywhere labels appear.
- Long labels are truncated with ellipsis; detailed metadata is shown in a detail pane or status line.
- Hover/tooltips are not a dependency (terminal UX).

**Rationale:** Terminal UIs lack reliable hover, and IDs are the most unambiguous cross-view reference.

**Revisit if:** We add a persistent details panel or toggled “verbose labels” mode that can replace truncation.

### 3) Topology fidelity

**Decision:** Stable, semantic layout (deterministic), not force-directed.

- Use deterministic ordering and semantic positioning (e.g., workflow tiers or adjacency ordering).
- Avoid physics-based layouts that jitter across updates.

**Rationale:** Stability beats visual fidelity in a text terminal; jitter breaks scanning and makes deltas hard to read.

**Revisit if:** We add a dedicated “explore” mode with user-controlled layout and persistence.

### 4) Scrub semantics

**Decision:** Default scrub scope is per-cluster; per-agent in agent-focused views.

- Cluster screen: scrub/scroll/timeline navigation applies to the cluster aggregate view.
- Agent screen: scrub/scroll is per-agent by default.
- Scope never switches implicitly; it follows the active screen/focus.

**Rationale:** Matches user intent in each view and avoids hidden scope changes during navigation.

**Revisit if:** We add multi-pane synchronized scrubbing or explicit cross-filtering controls.

### 5) Spine height

**Decision:** Strict 1-line spine; no persistent second hint line.

- Core spine UI remains one line for maximum vertical space.
- Hints and transient guidance use the toast/status area instead of expanding the spine.

**Rationale:** Preserves vertical space for live data and avoids layout reflow during bursts of guidance.

**Revisit if:** Onboarding or accessibility testing shows a persistent hint line materially improves success rates.

## Decision Outputs (for M3 issues)

All M3 implementation issues should reference these decisions and keep behavior aligned unless a follow-up ADR explicitly revises them.
