UPDATE THIS FILE when making architectural changes, adding patterns, or changing conventions.

# Zeroshot: Multi-Agent Coordination Engine

Operational rules and references for automated agents working on this repo. Install:
`npm i -g @covibes/zeroshot` or `npm link` (dev).

## CRITICAL RULES

- Never spawn without permission. Do not run `zeroshot run <id>` unless the user explicitly asks to run it.
- Never use git in validator prompts. Validate files directly.
- Never ask questions. Agents run non-interactively; make autonomous decisions.
- Never edit `CLAUDE.md` unless explicitly asked to update docs.

Worker git operations are allowed only with isolation (`--worktree`, `--docker`, `--pr`, `--ship`). They are forbidden without isolation.

Read-only safe commands: `zeroshot list`, `zeroshot status`, `zeroshot logs`

Destructive commands (need permission): `zeroshot kill`, `zeroshot clear`, `zeroshot purge`

## Where to Look

| Concept                  | File                                |
| ------------------------ | ----------------------------------- |
| Conductor classification | `src/conductor-bootstrap.js`        |
| Base templates           | `cluster-templates/base-templates/` |
| Message bus              | `src/message-bus.js`                |
| Ledger (SQLite)          | `src/ledger.js`                     |
| Trigger evaluation       | `src/logic-engine.js`               |
| Agent wrapper            | `src/agent-wrapper.js`              |
| Providers registry       | `src/providers/index.js`            |
| Provider implementations | `src/providers/`                    |
| Provider detection       | `lib/provider-detection.js`         |
| Provider capabilities    | `src/providers/capabilities.js`     |
| TUI dashboard            | `src/tui/`                          |
| Docker mounts/env        | `lib/docker-config.js`              |
| Container lifecycle      | `src/isolation-manager.js`          |
| Settings                 | `lib/settings.js`                   |

## CLI Quick Reference

```bash
# Flag cascade: --ship -> --pr -> --worktree
zeroshot run 123                  # Local, no isolation
zeroshot run 123 --worktree       # Git worktree isolation
zeroshot run 123 --pr             # Worktree + create PR
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
zeroshot watch                    # TUI dashboard
zeroshot export <id>              # Export conversation
zeroshot agents list              # Available agents
zeroshot settings                 # View/modify settings
zeroshot providers                # Provider status and defaults
```

UX modes:

- Foreground (`zeroshot run`): streams logs, Ctrl+C stops cluster.
- Daemon (`-d`): background, Ctrl+C detaches.
- Attach (`zeroshot attach`): connect to daemon, Ctrl+C detaches only.

Settings: `defaultProvider`, `providerSettings` (claude/codex/gemini), legacy `maxModel`, `defaultConfig`, `logLevel`.

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

Workers are now explicitly ordered to treat every `VALIDATION_RESULT` line as non-negotiable law before typing again. Failing to read and address each validator complaint before claiming completion will be rejected automatically.

## CI Failure Diagnosis

Multiple CI jobs fail → Diagnose each independently.

1. Get exact status: `gh api repos/covibes/zeroshot/actions/runs/{RUN_ID}/jobs`
2. Read ACTUAL error: `gh api repos/covibes/zeroshot/actions/jobs/{JOB_ID}/logs`
3. Fix ONE error → Push → Rerun → Repeat

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
