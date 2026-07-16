UPDATE THIS FILE when making architectural changes, adding patterns, or changing conventions.

# Zeroshot: Multi-Agent Coordination Engine

Operational rules and references for automated agents working on this repo. Install:
`npm i -g @the-open-engine/zeroshot` or `npm link` (dev).

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

| Concept                     | File                                                               |
| --------------------------- | ------------------------------------------------------------------ |
| Conductor classification    | `src/conductor-bootstrap.js`                                       |
| Base templates              | `cluster-templates/base-templates/`                                |
| Message bus                 | `src/message-bus.js`                                               |
| Ledger (SQLite)             | `src/ledger.js`                                                    |
| Guidance topics             | `src/guidance-topics.js`                                           |
| Guidance mailbox helper     | `src/ledger.js`                                                    |
| Guidance live injection     | `src/orchestrator.js`                                              |
| Trigger evaluation          | `src/logic-engine.js`                                              |
| Agent wrapper               | `src/agent-wrapper.js`                                             |
| Providers registry          | `src/providers/index.js`                                           |
| Provider implementations    | `src/providers/`                                                   |
| Provider engine registry    | `src/agent-cli-provider/provider-registry.ts`                      |
| Gateway runner              | `src/agent-cli-provider/gateway-runner.ts`                         |
| Gateway tools/policy        | `src/agent-cli-provider/gateway-tools.ts`                          |
| Provider detection          | `lib/provider-detection.js`                                        |
| Provider capabilities       | `src/providers/capabilities.js`                                    |
| Start-cluster helper        | `lib/start-cluster.js`                                             |
| Docker mounts/env           | `lib/docker-config.js`                                             |
| Container lifecycle         | `src/isolation-manager.js`                                         |
| Settings                    | `lib/settings.js`                                                  |
| Cluster wire/domain types   | `crates/openengine-cluster-protocol/`                              |
| Admission wire semantics    | `crates/openengine-cluster-protocol/src/admission.rs`              |
| Graph AST/bindings/guards   | `crates/openengine-cluster-protocol/src/graph.rs`                  |
| Closed payload algebra      | `crates/openengine-cluster-protocol/src/payload.rs`                |
| Closed payload validation   | `crates/openengine-cluster-protocol/src/payload_value.rs`          |
| Compiled IR/identity        | `crates/openengine-cluster-protocol/src/canonical.rs`              |
| Artifact receipts           | `crates/openengine-cluster-protocol/src/artifact.rs`               |
| Graph diagnostics/bounds    | `crates/openengine-cluster-protocol/src/diagnostic.rs`             |
| Shared wire-value bounds    | `crates/openengine-cluster-protocol/src/value.rs`                  |
| Cluster dispatch/stdio      | `crates/openengine-cluster-server/`                                |
| Native product construction | `zeroshot-rust/`                                                   |
| Admission coordinator       | `crates/openengine-cluster-server/src/admission.rs`                |
| Admission durable ports     | `crates/openengine-cluster-server/src/admission/ports.rs`          |
| Admission snapshot folding  | `crates/openengine-cluster-server/src/admission/snapshot.rs`       |
| Lifecycle state machine     | `crates/openengine-cluster-server/src/lifecycle.rs`                |
| Lifecycle durable ports     | `crates/openengine-cluster-server/src/lifecycle/ports.rs`          |
| Cluster typed transports    | `crates/openengine-cluster-client/`                                |
| Cluster fixtures/artifacts  | `crates/openengine-cluster-testkit/`                               |
| Scripted admission fixtures | `crates/openengine-cluster-testkit/src/admission.rs`               |
| Fixture inspection controls | `crates/openengine-cluster-testkit/src/admission/inspection.rs`    |
| Scripted lifecycle helpers  | `crates/openengine-cluster-testkit/src/lifecycle.rs`               |
| Lifecycle fixture params    | `crates/openengine-cluster-testkit/src/lifecycle/params.rs`        |
| Admission transcript output | `crates/openengine-cluster-testkit/src/admission_artifacts.rs`     |
| Negative graph vectors      | `crates/openengine-cluster-testkit/src/negative_graph_fixtures.rs` |
| Graph contract prose        | `docs/openengine-cluster-protocol/v1/graph-contract.md`            |
| Admission contract prose    | `docs/openengine-cluster-protocol/v1/admission.md`                 |
| Lifecycle contract prose    | `docs/openengine-cluster-protocol/v1/lifecycle.md`                 |
| Generated graph fixtures    | `protocol/openengine-cluster/v1/fixtures/graph/`                   |

Cluster Protocol Rust types are the source of truth. Files under
`protocol/openengine-cluster/v1/` are generated projections; update them with
`cargo run -p openengine-cluster-testkit --bin generate-cluster-protocol -- --write` and
verify byte-for-byte drift with `npm run protocol:check`. These generator-formatted artifacts
are excluded from Prettier; never format them independently.
The protocol and server crates own wire contracts, backend traits, the dispatcher, and transports.
`zeroshot-rust/` owns only the concrete `NativeBackend` and product-local `NativeBackendFactory`
construction root; keep protocol, transport, daemon, and compatibility behavior outside it.
Graph syntax, payload subtyping, compiled IR, diagnostics, and artifact receipt Rust types are
authoritative contract types only. They do not provide graph admission, verification, or execution.
The admission coordinator provides stateful plan/apply/get semantics through injected ports.
Testkit scripted approval and `running` phase mean admitted state, not native verification or a
production full-graph executor.
Authoritative admission snapshots fail closed: `empty` has no durable fields, `running` has the
complete matching control/seed tuple, and transient `admitting` preserves one of those two shapes.
Operational suspend is a dispatch gate: existing leases may land verified I/O, but successors wait
for resume. Drain waits without inventing graph hooks; force cancels and voids leases without
fabricating output. Each stopped run has one final `finished` event. Stop acknowledgements never
claim rollback or absence of external side effects. These are deterministic scripted-backend
semantics, not a native graph scheduler or worker executor.

The TUI is not included in this release. Use `zeroshot list`, `zeroshot status <id>`,
and `zeroshot logs <id> -f` or `zeroshot logs <id> -w` for monitoring.

### Cluster Worker Contracts

| Concept                    | File                                                       |
| -------------------------- | ---------------------------------------------------------- |
| Worker descriptors         | `crates/openengine-cluster-protocol/src/worker.rs`         |
| Normalized worker outcomes | `crates/openengine-cluster-protocol/src/worker/outcome.rs` |
| Worker registry boundary   | `crates/openengine-cluster-server/src/worker_registry.rs`  |
| Mock worker profiles       | `crates/openengine-cluster-testkit/src/worker_profiles.rs` |

Worker descriptors and registry compatibility checks are contract/pre-admission ports only.
ACP/A2A modules in the testkit are mock conformance profiles, never production transports.
Descriptors must declare all four closed runtime errors (`timeout`, `crash`, `malformed`, `refusal`).
The reserved legacy descriptor is valid only with its canonical request/result payload types, while
mock verifier completions must validate output, signals, diagnostics, and artifacts before emission.
Worker JSON Schema must mirror descriptor cross-field/uniqueness validation and the closed
error-code/reason matrix; registry compatibility must reject verifier contracts on step nodes.

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
zeroshot logs <id> [-f|-w]        # Stream logs
zeroshot resume <id> [prompt]     # Resume failed cluster
zeroshot stop <id>                # Graceful stop
zeroshot kill <id>                # Force kill

# Utilities
zeroshot export <id>              # Export conversation
zeroshot agents list              # Available agents
zeroshot settings                 # View/modify settings
zeroshot providers                # Provider status and defaults
```

UX modes:

- Foreground (`zeroshot run`): streams logs, Ctrl+C stops cluster.
- Daemon (`-d`): background, Ctrl+C detaches.
- Attach (`zeroshot attach`): connect to daemon, Ctrl+C detaches only.

Settings: `defaultProvider`, `providerSettings` (claude/codex/gateway/gemini/opencode/pi/copilot), legacy `maxModel`, `defaultConfig`, `logLevel`, robustness (`maxRetries`, `backoffBaseMs`, `backoffMaxMs`, `jitterFactor`, `maxRestartAttempts`, `maxTotalRestarts`, `staleWarningsBeforeKill`).

Provider engines are registry-owned: adding an engine means one entry in `src/agent-cli-provider/provider-registry.ts`, plus the provider-specific adapter and tests. Docker credential mount/env presets, CLI aliases, visible preset lists, and any nontrivial availability probe rules must derive from that registry entry; do not add new provider identity lists or provider preset lists elsewhere.
Model gateways stay behind the single bundled `gateway` engine. Do not add `openrouter`, `ollama`, `vllm`, `hermes`, or similar model-only targets as standalone provider ids.

ACP-native engines use one shared stdio adapter lane. New ACP engines must be added with registry metadata plus helper fixtures only; do not add engine-specific ACP parsers or invoke runners.
ACP fixtures must use protocol-shaped chunk payloads: `agent_message_chunk.content` is a single `ContentBlock` object, and thought deltas are covered with `agent_thought_chunk` fixtures so parser tests catch spec drift.

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
- Provider names use CLI identifiers: `claude`, `codex`, `gemini`, `opencode`, `pi`, `copilot` (legacy `anthropic`/`openai`/`google` map to these).
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

Mount presets: infrastructure presets plus provider ids from `src/agent-cli-provider/provider-registry.ts`.

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

1. Get exact status: `gh api repos/the-open-engine/zeroshot/actions/runs/{RUN_ID}/jobs`
2. Read ACTUAL error: `gh api repos/the-open-engine/zeroshot/actions/jobs/{JOB_ID}/logs`
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
