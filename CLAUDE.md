# Zeroshot: Multi-Agent Coordination Engine

Message-passing primitives for multi-agent workflows. **Install:** `npm i -g @covibes/zeroshot` or `npm link` (dev).

## 🔴 CRITICAL RULES

| Rule | Why | Forbidden | Required |
|------|-----|-----------|----------|
| **Never spawn without permission** | Consumes API credits | "I'll run zeroshot on 123" | User says "run zeroshot" |
| **Never use git in validators** | Git state unreliable | `git diff`, `git status` in prompts | Validate files directly |
| **Never ask questions** | Agents run non-interactively | `AskUserQuestion`, waiting for confirmation | Make autonomous decisions |
| **Never edit AI instruction files** | Config files for other assistants | Editing `AGENTS.md`, `GEMINI.md` | Read-only unless explicitly asked |

**Worker git operations:** Allowed with isolation (`--worktree`, `--docker`, `--pr`, `--ship`). Forbidden without isolation (auto-injected restriction).

**Read-only safe:** `zeroshot list`, `zeroshot status`, `zeroshot logs`

**Destructive (needs permission):** `zeroshot kill`, `zeroshot clear`, `zeroshot purge`

## Where to Look

| Concept | File |
|---------|------|
| Conductor classification | `src/conductor-bootstrap.js` |
| Base templates | `cluster-templates/base-templates/` |
| Message bus | `src/message-bus.js` |
| Ledger (SQLite) | `src/ledger.js` |
| Trigger evaluation | `src/logic-engine.js` |
| Agent wrapper | `src/agent-wrapper.js` |
| TUI dashboard | `src/tui/` |

## CLI Quick Reference

```bash
# Flag cascade: --ship → --pr → --worktree
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
```

**UX modes:**
- Foreground (`zeroshot run`): Streams logs, Ctrl+C **stops** cluster
- Daemon (`-d`): Background, Ctrl+C detaches
- Attach (`zeroshot attach`): Connect to daemon, Ctrl+C **detaches** only

**Settings:** `maxModel` (opus/sonnet/haiku cost ceiling), `defaultConfig`, `logLevel`

## Architecture

**Pub/sub message bus + SQLite ledger.** Agents subscribe to topics, execute on trigger match, publish results.

```
Agent A → publish() → SQLite Ledger → LogicEngine → trigger match → Agent B executes
```

### Core Primitives

| Primitive | Purpose |
|-----------|---------|
| Topic | Named message channel (`ISSUE_OPENED`, `VALIDATION_RESULT`) |
| Trigger | Condition to wake agent (`{ topic, action, logic }`) |
| Logic Script | JS predicate for complex conditions |
| Hook | Post-task action (publish message, execute command) |

### Agent Configuration (Minimal)

```json
{
  "id": "worker",
  "role": "implementation",
  "model": "sonnet",
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

### Logic Script API

```javascript
// Ledger (auto-scoped to cluster)
ledger.query({ topic, sender, since, limit })
ledger.findLast({ topic })
ledger.count({ topic })

// Cluster
cluster.getAgents()
cluster.getAgentsByRole('validator')

// Helpers
helpers.allResponded(agents, topic, since)
helpers.hasConsensus(topic, since)
```

## Conductor: 2D Classification

Classifies tasks on **Complexity × TaskType**, routes to parameterized templates.

| Complexity | Description | Validators |
|------------|-------------|------------|
| TRIVIAL | 1 file, mechanical | 0 |
| SIMPLE | 1 concern | 1 |
| STANDARD | Multi-file | 3 |
| CRITICAL | Auth/payments/security | 5 |

| TaskType | Action |
|----------|--------|
| INQUIRY | Read-only exploration |
| TASK | Implement new feature |
| DEBUG | Fix broken code |

**Base templates:** `single-worker`, `worker-validator`, `debug-workflow`, `full-workflow`

## Isolation Modes

| Mode | Flag | Use When |
|------|------|----------|
| Worktree | `--worktree` | Quick isolated work, PR workflows |
| Docker | `--docker` | Full isolation, risky experiments, parallel agents |

**Worktree:** Lightweight git branch isolation (<1s setup).

**Docker:** Fresh git clone in container, credentials mounted, auto-cleanup.

## Adversarial Tester (STANDARD+ only)

**Core principle:** Tests passing ≠ implementation works. The ONLY verification is: **USE IT YOURSELF.**

1. Read issue → understand requirements
2. Look at code → figure out how to invoke
3. Run it → did it work?
4. Try to break it → edge cases
5. Verify each requirement → evidence (command + output)

## Persistence

| File | Content |
|------|---------|
| `~/.zeroshot/clusters.json` | Cluster metadata |
| `~/.zeroshot/<id>.db` | SQLite message ledger |

Clusters survive crashes. Resume: `zeroshot resume <id>`

## Known Limitations

**Bash subprocess output not streamed:** Claude CLI returns `tool_result` after subprocess completes. Long scripts show no output until done.

## Mechanical Enforcement

| Antipattern | Enforcement |
|-------------|-------------|
| Dangerous fallbacks | ESLint ERROR |
| Manual git tags | Pre-push hook |
| Git in validator prompts | Config validator |
| Multiple impl files (-v2) | Pre-commit hook |
