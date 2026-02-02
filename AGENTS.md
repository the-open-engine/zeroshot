UPDATE THIS FILE when making architectural changes, adding patterns, or changing conventions.

# Zeroshot: Multi-Agent Coordination Engine

Operational rules and references for automated agents working on this repo. Install:
`npm i -g @covibes/zeroshot` or `npm link` (dev).

## CRITICAL RULES

- Never spawn without permission. Do not run `zeroshot run <id>` unless the user explicitly asks to run it.
- Never use git in validator prompts. Validate files directly.
- Never ask questions. Agents run non-interactively; make autonomous decisions.
- Never edit `CLAUDE.md` unless explicitly asked to update docs.
- Detached (`-d`) runs must forward all `zeroshot run` options via `ZEROSHOT_RUN_OPTIONS` (see `buildDaemonEnv` + `buildStartOptions`) so PR/worktree config cannot be dropped.

Worker git operations are allowed only with isolation (`--worktree`, `--docker`, `--pr`, `--ship`). They are forbidden without isolation.

Read-only safe commands: `zeroshot list`, `zeroshot status`, `zeroshot logs`

Destructive commands (need permission): `zeroshot kill`, `zeroshot clear`, `zeroshot purge`

## Where to Look

| Concept                        | File                                                       |
| ------------------------------ | ---------------------------------------------------------- |
| Conductor classification       | `src/conductor-bootstrap.js`                               |
| Base templates                 | `cluster-templates/base-templates/`                        |
| Message bus                    | `src/message-bus.js`                                       |
| Ledger (SQLite)                | `src/ledger.js`                                            |
| Guidance topics                | `src/guidance-topics.js`                                   |
| Guidance mailbox helper        | `src/ledger.js`                                            |
| Guidance live injection        | `src/orchestrator.js`                                      |
| Trigger evaluation             | `src/logic-engine.js`                                      |
| Agent wrapper                  | `src/agent-wrapper.js`                                     |
| Providers registry             | `src/providers/index.js`                                   |
| Provider implementations       | `src/providers/`                                           |
| Provider detection             | `lib/provider-detection.js`                                |
| Provider capabilities          | `src/providers/capabilities.js`                            |
| TUI backend entrypoint         | `src/tui-backend/index.ts`                                 |
| TUI backend server             | `src/tui-backend/server.ts`                                |
| TUI backend services           | `src/tui-backend/services/`                                |
| TUI backend subscriptions      | `src/tui-backend/subscriptions/`                           |
| TUI backend build output       | `lib/tui-backend/`                                         |
| TUI launcher (Node)            | `lib/tui-launcher.js`                                      |
| TUI binary mapping             | `lib/tui-binary.js`                                        |
| TUI start-cluster helper       | `lib/start-cluster.js`                                     |
| TUI binary installer           | `scripts/install-tui-binary.js`                            |
| TUI v2 protocol spec           | `docs/tui-v2/protocol.md`                                  |
| TUI v2 protocol types (TS)     | `src/tui-backend/protocol/`                                |
| TUI v2 protocol types (Rust)   | `tui-rs/crates/zeroshot-tui/src/protocol/`                 |
| Rust TUI backend client        | `tui-rs/crates/zeroshot-tui/src/backend/`                  |
| Rust TUI entrypoint            | `tui-rs/crates/zeroshot-tui/src/main.rs`                   |
| Rust TUI core loop (MVU)       | `tui-rs/crates/zeroshot-tui/src/app/mod.rs`                |
| Rust TUI spine completion      | `tui-rs/crates/zeroshot-tui/src/app/spine_completion.rs`   |
| Rust TUI input routing         | `tui-rs/crates/zeroshot-tui/src/input.rs`                  |
| Rust TUI commands              | `tui-rs/crates/zeroshot-tui/src/commands/`                 |
| Rust TUI command parser        | `tui-rs/crates/zeroshot-tui/src/commands/parser.rs`        |
| Rust TUI command dispatch      | `tui-rs/crates/zeroshot-tui/src/commands/dispatcher.rs`    |
| Rust TUI command types         | `tui-rs/crates/zeroshot-tui/src/commands/types.rs`         |
| Rust TUI screens               | `tui-rs/crates/zeroshot-tui/src/screens/`                  |
| Rust TUI Fleet Radar screen    | `tui-rs/crates/zeroshot-tui/src/screens/radar.rs`          |
| Rust TUI Cluster Canvas screen | `tui-rs/crates/zeroshot-tui/src/screens/cluster_canvas.rs` |
| Rust TUI render entrypoint     | `tui-rs/crates/zeroshot-tui/src/ui/mod.rs`                 |
| Rust TUI widgets               | `tui-rs/crates/zeroshot-tui/src/ui/widgets/`               |
| Rust TUI toast widget          | `tui-rs/crates/zeroshot-tui/src/ui/widgets/toast.rs`       |
| Rust TUI command bar widget    | `tui-rs/crates/zeroshot-tui/src/ui/widgets/command_bar.rs` |
| Rust TUI terminal guard        | `tui-rs/crates/zeroshot-tui/src/terminal.rs`               |
| Docker mounts/env              | `lib/docker-config.js`                                     |
| Container lifecycle            | `src/isolation-manager.js`                                 |
| Settings                       | `lib/settings.js`                                          |

TUI v2 (Rust) convention:
Ratatui is the only supported TUI; legacy UI removed. centralized key routing in `src/input.rs`; `app::update()` is pure and returns effects; `ui::render()` is pure and performs no IO. Adding a screen requires a `ScreenId` variant plus a screen reducer and render entry.
TUI v2 (Rust) command flow: `Effect::Command(CommandRequest)` is emitted by `app::update()` and executed in `src/main.rs` via `commands::dispatch()`, with failures surfaced through `BackendAction::Error`.
TUI v2 (Rust) provider override lives in `AppState.provider_override` and is forwarded when launching clusters (e.g. `StartClusterFromText`).
TUI v2 (Rust) command bar: `AppState.command_bar` captures input; `/` opens it outside Launcher; Esc closes; Submit dispatches. Toast output lives in `AppState.toast` and renders via `ui/widgets/toast.rs`.
TUI v2 (Rust) Agent Microscope renders phase markers derived from cluster timeline events (deduped, capped) in a left margin when space allows.
TUI v2 (Rust) Disruptive zoom stack: `ScreenId::IntentConsole` (root), `FleetRadar`, `ClusterCanvas { id }`, `AgentMicroscope { cluster_id, agent_id }`; zoom stack context drives spine whisper targets.
TUI v2 (Rust) Cluster Canvas overlays: use `ui/widgets/stream.rs` StreamOverlay + placement helper in `screens/cluster_canvas.rs` to render bounded log/timeline slices near focus, clamped to canvas bounds and never intersecting the spine; render after the canvas draw.
TUI v2 (Rust) calm empty states: use `ui/shared.rs::calm_empty_state` for centered headline/detail/footer cards in Disruptive screens.
TUI v2 (Rust) Disruptive stream windowing: `TimeCursor` (mode, `t_ms`, `window_ms`) plus `TimeIndexedBuffer` in `ui/shared.rs` back logs/timeline window queries; cluster canvas overlay renders windowed slices from time-indexed buffers.
TUI v2 (Rust) motion/smoothing: `app/animation.rs` defines `AnimClock` + smoothing helpers; `AppState.anim_clock` + `last_tick_ms` advance on `Tick`; Fleet Radar smooths orb radius/intensity in `FleetRadarState.orb_states` and uses `pulse_factor` for error pulses; Cluster Canvas uses camera target/velocity smoothing via `State.tick_camera()` (render consumes smoothed camera).
TUI v2 (Rust) spine intent submit detects issue refs (`123`, `owner/repo#123`, GitHub issue URL) → `StartClusterFromIssue`; otherwise `StartClusterFromText`.
TUI v2 (Rust) Disruptive pre-M3 decisions live in `docs/ZEROSHOT-DISRUPTIVE-TUI-DECISIONS.md` (focus, labels, topology, scrub, spine height).
TUI backend test envs: `ZEROSHOT_TUI_BACKEND_MOCK_LAUNCH`, `ZEROSHOT_TUI_BACKEND_MOCK_GUIDANCE`, `ZEROSHOT_TUI_BACKEND_METRICS_PLATFORM` (override platform for metrics; unsupported values force `supported=false`).
TUI backend path override: `ZEROSHOT_TUI_BACKEND_PATH`.
TUI launcher env: `ZEROSHOT_TUI_BINARY_PATH` overrides the installed Rust binary, `ZEROSHOT_TUI_PATH`/`ZEROSHOT_TUI_BIN` override Rust binary path, `ZEROSHOT_TUI_BINARY_URL` overrides release asset URL, `ZEROSHOT_TUI_BINARY_SKIP` skips download, `ZEROSHOT_TUI_INITIAL_SCREEN` + `ZEROSHOT_TUI_PROVIDER_OVERRIDE` + `ZEROSHOT_TUI_UI` feed Rust startup defaults (UI variants: classic, disruptive; CLI: `zeroshot tui --ui <variant>`).

## CLI Quick Reference

```bash
# Flag cascade: --ship -> --pr -> --worktree
zeroshot run 123                  # Local, no isolation
zeroshot run 123 --worktree       # Git worktree isolation
zeroshot run 123 --pr             # Worktree + create PR
zeroshot run 123 --pr --pr-base dev # PR base: dev, worktree base: origin/dev (incl. -d)
zeroshot run 123 --ship           # Worktree + PR + auto-merge
zeroshot run 123 --docker         # Docker container isolation
zeroshot run 123 -d               # Background (daemon) mode

# Management
zeroshot list                     # All clusters (--json)
zeroshot status <id>              # Cluster details
zeroshot logs <id> [-f]           # Stream logs
zeroshot resume <id> [prompt]     # Resume failed cluster
zeroshot stop <id>                # Graceful stop
zeroshot kill <id>                # Force kill

# Utilities
zeroshot                          # TUI (TTY only; Rust default)
zeroshot tui                      # TUI explicit entry
zeroshot watch                    # TUI Monitor view
zeroshot export <id>              # Export conversation
zeroshot agents list              # Available agents
zeroshot settings                 # View/modify settings
zeroshot providers                # Provider status and defaults
```

UX modes:

- Foreground (`zeroshot run`): streams logs, Ctrl+C stops cluster.
- Daemon (`-d`): background, Ctrl+C detaches.
- Attach (`zeroshot attach`): connect to daemon, Ctrl+C detaches only.

Settings: `defaultProvider`, `providerSettings` (claude/codex/gemini), legacy `maxModel`, `defaultConfig`, `logLevel`, robustness (`maxRetries`, `backoffBaseMs`, `backoffMaxMs`, `jitterFactor`, `maxRestartAttempts`, `maxTotalRestarts`, `staleWarningsBeforeKill`).

## Architecture

Pub/sub message bus + SQLite ledger. Agents subscribe to topics, execute on trigger match, publish results.

```
Agent A -> publish() -> SQLite Ledger -> LogicEngine -> trigger match -> Agent B executes
```

### Core Primitives

| Primitive    | Purpose                                                     |
| ------------ | ----------------------------------------------------------- |
| Topic        | Named message channel (`ISSUE_OPENED`, `VALIDATION_RESULT`) |
| Trigger      | Condition to wake agent (`{ topic, action, logic }`)        |
| Logic Script | JS predicate for complex conditions                         |
| Hook         | Post-task action (publish message, execute command)         |

Restart persistence: orchestrator publishes `AGENT_RESTART_ATTEMPT` to the ledger so restart limits survive orchestrator restarts.

### Guidance Messaging

- Topics: `USER_GUIDANCE_CLUSTER`, `USER_GUIDANCE_AGENT` (see `src/guidance-topics.js`).
- Mailbox helper: `ledger.queryGuidanceMailbox()` with `messageBus.queryGuidanceMailbox()` passthrough.
- Live injection: `Orchestrator.sendGuidanceToAgent()` uses `agent.injectInput()` to attempt PTY stdin; always persists `USER_GUIDANCE_AGENT` with `metadata.delivery` (`status: injected|unsupported`, `method: pty`, `taskId`, `reason`).
- Safe-point queue fallback: `AgentWrapper._buildContext()` pulls queued guidance via `collectQueuedGuidance()` and injects a delimited block in `agent-context-builder` between Instructions and Output Schema. Cursor: `agent.lastGuidanceAppliedAt`.

### Agent Configuration (Minimal)

```json
{
  "id": "worker",
  "role": "implementation",
  "modelLevel": "level2",
  "triggers": [{ "topic": "ISSUE_OPENED", "action": "execute_task" }],
  "prompt": "Implement the requested feature...",
  "hooks": {
    "onComplete": {
      "action": "publish_message",
      "config": { "topic": "IMPLEMENTATION_READY" }
    }
  }
}
```

### Provider Model Levels

- Use `modelLevel` (`level1`/`level2`/`level3`) for provider-agnostic configs.
- Set `provider` per agent or `defaultProvider`/`forceProvider` at cluster level.
- Provider names use CLI identifiers: `claude`, `codex`, `gemini` (legacy `anthropic`/`openai`/`google` map to these).
- `model` remains a provider-specific escape hatch.
- Codex/Opencode only: `reasoningEffort` (`low|medium|high|xhigh`).

### Logic Script API

```javascript
// Ledger (auto-scoped to cluster)
ledger.query({ topic, sender, since, limit });
ledger.findLast({ topic });
ledger.count({ topic });

// Cluster
cluster.getAgents();
cluster.getAgentsByRole('validator');

// Helpers
helpers.allResponded(agents, topic, since);
helpers.hasConsensus(topic, since);
```

Context strategies now support `since: 'last_agent_start'` to scope history to the most recent
iteration start for the executing agent. Acceptable values: `cluster_start`, `last_task_end`,
`last_agent_start`, or an ISO timestamp string.

## Conductor: 2D Classification

Classifies tasks on Complexity x TaskType, routes to parameterized templates.

| Complexity | Description            | Validators |
| ---------- | ---------------------- | ---------- |
| TRIVIAL    | 1 file, mechanical     | 0          |
| SIMPLE     | 1 concern              | 1          |
| STANDARD   | Multi-file             | 3          |
| CRITICAL   | Auth/payments/security | 5          |

| TaskType | Action                |
| -------- | --------------------- |
| INQUIRY  | Read-only exploration |
| TASK     | Implement new feature |
| DEBUG    | Fix broken code       |

Base templates: `single-worker`, `worker-validator`, `debug-workflow`, `full-workflow`.

## Isolation Modes

| Mode     | Flag         | Use When                                           |
| -------- | ------------ | -------------------------------------------------- |
| Worktree | `--worktree` | Quick isolated work, PR workflows                  |
| Docker   | `--docker`   | Full isolation, risky experiments, parallel agents |

Worktree: lightweight git branch isolation (<1s setup).
Docker: fresh git clone in container, credentials mounted, auto-cleanup.

## Docker Mount Configuration

Configurable credential mounts for `--docker` mode. See `lib/docker-config.js`.

| Setting                | Type          | Default  | Description                                           |
| ---------------------- | ------------- | -------- | ----------------------------------------------------- | ---------------------------------------- |
| `dockerMounts`         | `Array<string | object>` | `['gh','git','ssh']`                                  | Presets or `{host, container, readonly}` |
| `dockerEnvPassthrough` | `string[]`    | `[]`     | Extra env vars (supports `VAR`, `VAR_*`, `VAR=value`) |
| `dockerContainerHome`  | `string`      | `/root`  | Container home for `$HOME` expansion                  |

Mount presets: `gh`, `git`, `ssh`, `aws`, `azure`, `kube`, `terraform`, `gcloud`, `claude`, `codex`, `gemini`, `opencode`.

Provider CLIs in Docker require credential mounts; Zeroshot warns when missing.

Env var syntax:

- `VAR` -> pass if set in host env
- `VAR_*` -> pass all matching (e.g., `TF_VAR_*`)
- `VAR=value` -> always set to value
- `VAR=` -> always set to empty string

Config priority: CLI flags > `ZEROSHOT_DOCKER_MOUNTS` env > settings > defaults.

```bash
# Persistent config
zeroshot settings set dockerMounts '["gh","git","ssh","aws"]'

# Per-run override
zeroshot run 123 --docker --mount ~/.custom:/root/.custom:ro

# Disable all mounts
zeroshot run 123 --docker --no-mounts
```

## Adversarial Tester (STANDARD+ only)

Core principle: tests passing != implementation works. The ONLY verification is: USE IT YOURSELF.

1. Read issue -> understand requirements
2. Look at code -> figure out how to invoke
3. Run it -> did it work?
4. Try to break it -> edge cases
5. Verify each requirement -> evidence (command + output)

## Persistence

| File                        | Content               |
| --------------------------- | --------------------- |
| `~/.zeroshot/clusters.json` | Cluster metadata      |
| `~/.zeroshot/<id>.db`       | SQLite message ledger |

Clusters survive crashes. Resume: `zeroshot resume <id>`.

## Known Limitations

Bash subprocess output not streamed: Claude CLI returns `tool_result` after subprocess completes.
Long scripts show no output until done.

### Kubernetes / Network Storage (SQLite Ledger)

Zeroshot’s message ledger is SQLite (`~/.zeroshot/<id>.db`). On Kubernetes, putting this on a
network filesystem (EFS/NFS/CephFS) can cause severe latency and lock contention.

Mitigations (env vars):

- `ZEROSHOT_SQLITE_JOURNAL_MODE=DELETE` (or `TRUNCATE`) for network filesystems that don’t like WAL
- `ZEROSHOT_SQLITE_WAL_AUTOCHECKPOINT_PAGES=1000` (default) to avoid per-write checkpoint storms
- `ZEROSHOT_SQLITE_BUSY_TIMEOUT_MS=5000` (default) to reduce `SQLITE_BUSY` flakiness under contention

Operational rule: don’t run multiple pods against the same `~/.zeroshot` volume unless you
really know what you’re doing—SQLite is not a multi-writer, multi-node database.

## Fixed Bugs (Reference)

### Template Agent CWD Injection (2026-01-03)

Bug: `--ship` mode created worktree but template agents (planning, implementation, validator)
ran in main directory instead, polluting it with uncommitted changes.

Root cause: `_opAddAgents()` didn't inject cluster's worktree cwd into dynamically spawned
template agents. Initial agents got cwd via `startCluster()`, but template agents loaded
later via conductor classification missed it.

Fix: added cwd injection to `_opAddAgents()` and resume path in `orchestrator.js`.
Test: `tests/worktree-cwd-injection.test.js`.

### PR Mode Completion Hang (2026-01-15)

Bug: PR-mode clusters stayed running after PR creation/merge because no
`CLUSTER_COMPLETE` was ever published.

Root cause: `git-pusher` relied on `output.publishAfter` without an onComplete
hook, so the orchestrator never received the completion signal.

Fix: added `onComplete` publish of `CLUSTER_COMPLETE` in
`src/agents/git-pusher-agent.json`.
Test: `tests/integration/orchestrator-flow.test.js`.

## Enforcement Philosophy

**ENFORCE > DOCUMENT. If enforceable, don't document.**

Preference: Type system > ESLint > Pre-commit hook > Documentation

Error messages ARE the documentation. Write them with what + fix.

## Anti-Patterns (Zeroshot-Specific)

### 1. Running Zeroshot Without Permission

```bash
# ❌ FORBIDDEN
agent: "I'll run zeroshot on issue #123"
zeroshot run 123

# ✅ CORRECT
agent: "Would you like me to run zeroshot on issue #123?"
# Wait for user consent
```

WHY: Multi-agent runs consume significant API credits.

### 2. Git Commands in Validator Prompts

```bash
# ❌ FORBIDDEN
validator_prompt: "Run git diff to verify changes..."

# ✅ CORRECT
validator_prompt: "Read src/index.js and verify function exists..."
```

WHY: Multiple agents modify git state concurrently. Validator reads stale state.

### 3. Asking Questions in Autonomous Workflows

```javascript
// ❌ FORBIDDEN
await AskUserQuestion('Should I use approach A or B?');

// ✅ CORRECT
// Decision: Using approach A because requirement specifies X
```

WHY: Zeroshot agents run non-interactively.

### 4. Worker Git Operations Without Isolation

```bash
# ❌ FORBIDDEN
zeroshot run 123  # Pollutes main directory

# ✅ CORRECT
zeroshot run 123 --worktree  # Isolated
zeroshot run 123 --pr        # Worktree + PR
zeroshot run 123 --docker    # Full isolation
```

WHY: Prevents contamination, enables parallel work.

### 5. Using Git Stash

```bash
# ❌ FORBIDDEN
git stash  # Hides work from other agents

# ✅ CORRECT
git add -A && git commit -m "WIP: feature implementation"
git switch other-branch
```

WHY: WIP commits are visible, never lost, squashable.

## Behavioral Rules

### Git Workflow (Multi-Agent)

Use WIP commits instead of stashing:

```bash
git add -A && git commit -m "WIP: save work"  # Instead of git stash
git switch <branch>                            # Instead of git checkout
git restore <file>                             # Instead of git checkout --
```

### Test-First Workflow

Write tests BEFORE or WITH code:

```bash
touch src/new-feature.js
touch tests/new-feature.test.js  # FIRST
# Write failing tests → Implement → Pass
```

### Validation Workflow

Run validation for:

- Significant changes (>50 lines)
- Refactoring across files
- When user explicitly requests

Trust pre-commit hooks for trivial changes.

```bash
npm run lint
npm run test
```

Mocha config: `.mocharc.cjs` applies defaults; passing explicit `*.test.js` files on the CLI skips the default `tests/**/*.test.js` spec.

Workers are now explicitly ordered to treat every `VALIDATION_RESULT` line as non-negotiable law before typing again. Failing to read and address each validator complaint before claiming completion will be rejected automatically.

## CI Failure Diagnosis

Multiple CI jobs fail → Diagnose each independently.

1. Get exact status: `gh api repos/covibes/zeroshot/actions/runs/{RUN_ID}/jobs`
2. Read ACTUAL error: `gh api repos/covibes/zeroshot/actions/jobs/{JOB_ID}/logs`
3. Fix ONE error → Push → Rerun → Repeat

## Release Pipeline Convention

- Dev required checks: `check` only (merge queue).
- Main required checks: `check` + `install-matrix` (merge queue).
- Cross-platform `install-matrix` runs in CI for main only.

Do NOT assume single root cause.

## CLAUDE.md Writing Rules

**Scope:** Narrowest possible.

**Content Priority:**

1. CRITICAL gotchas (caused real bugs)
2. "Where to Look" routing tables
3. Anti-patterns with WHY
4. Commands/troubleshooting

**DELETE:** Tutorial content, directory trees, interface definitions

**Format:** Tables over prose, ❌/✅ examples with WHY

## Mechanical Enforcement

| Antipattern               | Enforcement      |
| ------------------------- | ---------------- |
| Dangerous fallbacks       | ESLint ERROR     |
| Manual git tags           | Pre-push hook    |
| Git in validator prompts  | Config validator |
| Multiple impl files (-v2) | Pre-commit hook  |
| Spawn without permission  | Runtime check    |
| Git stash usage           | Pre-commit hook  |
| Rust formatting drift     | Pre-commit hook  |
