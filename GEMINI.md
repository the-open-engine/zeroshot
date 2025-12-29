# Zeroshot: Multi-Agent Coordination Engine

Multi-agent coordination via message-passing primitives. User install: `npm i -g @covibes/zeroshot`. Dev install: `npm link` in zeroshot root directory.

## Where to Look

| Concept                           | Primary File/Location              |
| --------------------------------- | ---------------------------------- |
| Conductor 2D classification       | @src/conductor-bootstrap.js        |
| Base templates                    | @cluster-templates/base-templates/ |
| Message bus                       | @src/message-bus.js                |
| Ledger (SQLite)                   | @src/ledger.js                     |
| Logic engine (trigger evaluation) | @src/logic-engine.js               |
| Orchestrator                      | @src/orchestrator.js               |
| Agent wrapper                     | @src/agent-wrapper.js              |
| TUI dashboard                     | @src/tui/                          |

## 🔴 CRITICAL USAGE RULES

**NEVER spawn agents without explicit user permission**

- WHY: Consumes API credits, modifies code autonomously
- ❌ FORBIDDEN: "I'll run zeroshot on issue 123" (no user request)
- ✅ OK: User says "run zeroshot", "spawn cluster", "start task"
- ❌ FORBIDDEN: `zeroshot kill`, `zeroshot clear` (destroys work/data)
- ✅ OK: `zeroshot list`, `zeroshot status`, `zeroshot logs` (read-only)

**NEVER use git operations in validator agents**

- WHY: Git state unreliable (dirty, uncommitted, non-existent)
- ❌ FORBIDDEN: `git diff`, `git status`, `git log` in prompts
- ✅ REQUIRED: Validate what agent reads directly (files, tests, implementation)

**NEVER ask questions from zeroshot agents**

- WHY: Agents run non-interactively, no user to respond, causes task failure
- ❌ FORBIDDEN: `AskUserQuestion`, "Would you like me to...", "Should I..."
- ❌ FORBIDDEN: Waiting for approval/confirmation
- ✅ REQUIRED: Make autonomous decisions
- ✅ REQUIRED: When choosing between "fix code" vs "relax rules" → ALWAYS fix the code

**Enforcement (defense in depth):**

1. **PreToolUse hook** - `hooks/block-ask-user-question.py` returns `permissionDecision: deny`
2. **Prompt injection** - `agent-wrapper.js:670-680` adds explicit NEVER ASK instructions
3. **Isolation mode** - `isolation-manager.js:631-659` creates Claude config with hook
4. **Non-isolation mode** - `agent-wrapper.js:40-88` creates zeroshot-specific Claude config via `CLAUDE_CONFIG_DIR`

**NEVER edit other AI instruction files**

- WHY: These are configuration files for other AI assistants, not for Claude
- ❌ FORBIDDEN: Editing `AGENTS.md`, `GEMINI.md`, or similar AI-specific files
- ✅ OK: Reading them for context if relevant to the task
- ✅ OK: Editing only if user explicitly requests changes to those specific files

## zeroshot CLI (Unified)

**Packages:** `zeroshot` (CommonJS cluster + ESM task via dynamic import)
**Commands work for both clusters and single-agent tasks automatically.**

**Commands:**

```bash
# Full automation
zeroshot auto 123                    # Isolated + auto-merge PR (recommended)
zeroshot auto 123 -d                 # Same, but detached/background

# Clusters (multi-agent)
zeroshot run 123                     # Issue number (auto-attaches to first agent)
zeroshot run 123 -d                  # Detached/background mode
zeroshot run "Implement X"           # Plain text
zeroshot run 123 --isolation         # Docker isolation
zeroshot run 123 --isolation --pr    # Isolation + create PR on success

# Single tasks
zeroshot task run "Fix bug X"        # Background single agent

# Both
zeroshot list / ls                   # All tasks/clusters
zeroshot status <id>                 # Auto-detects type
zeroshot logs <id> [-f]              # Follow logs
zeroshot resume <id> [prompt]        # Resume failed (foreground, Ctrl+C stops)
zeroshot resume <id> -d              # Resume in background (daemon mode)
zeroshot kill <id>                   # Kill running
zeroshot clear [-y]                  # Kill all + delete data

# Cluster-only
zeroshot stop <id>                   # Graceful shutdown
zeroshot export <id>                 # Export conversation
zeroshot config list/show <name>     # Manage configs
zeroshot watch                       # TUI dashboard (htop-style)

# Agent Library
zeroshot agents list / ls            # View available agent definitions
zeroshot agents list --verbose       # Show full agent details
zeroshot agents list --json          # Output as JSON

# Settings
zeroshot settings                    # Show all (highlights non-defaults)
zeroshot settings set maxModel sonnet
```

**Settings:** `maxModel` (opus/sonnet/haiku - cost ceiling), `defaultConfig`, `defaultIsolation`, `logLevel`

**maxModel (Cost Ceiling):**
- Sets the maximum model agents can request (not a default/override)
- Agent requests opus but maxModel is sonnet → **ERROR** at config time
- Agent requests sonnet with maxModel opus → OK (within ceiling)
- Agent unspecified → uses maxModel as default

**UX:**

- `zeroshot run` = Foreground mode (streams logs, Ctrl+C **STOPS** cluster)
- `zeroshot run -d` = Daemon mode (background, like docker -d)
- `zeroshot resume` = Foreground mode (streams logs, Ctrl+C **STOPS** cluster)
- `zeroshot resume -d` = Daemon mode (background)
- `zeroshot attach` = Connect to running daemon (Ctrl+C **DETACHES**, daemon continues)
- `zeroshot task run` = Single-agent background task

**FAQ: Can I run zeroshot in the background?**

| Question | Answer |
|----------|--------|
| **How do I run in background?** | Use `-d` flag: `zeroshot run 123 -d` or `zeroshot resume xyz -d` |
| **What happens if I Ctrl+C?** | **Foreground (no -d)**: Cluster **STOPS** completely. **Attach mode**: You **DETACH**, daemon continues. |
| **How do I follow logs of a background cluster?** | `zeroshot logs <id> -f` |
| **How do I stop a background cluster?** | `zeroshot stop <id>` (graceful) or `zeroshot kill <id>` (force) |
| **Can I resume a stopped cluster?** | Yes: `zeroshot resume <id>` (foreground) or `zeroshot resume <id> -d` (background) |

## Architecture: Message-Driven Coordination

**Pub/sub message bus + immutable SQLite ledger.** Agents subscribe to topics, execute on trigger match, publish results via hooks. Creates decoupled event-driven workflows.

### Primitives

| Primitive        | Purpose                                     | Example                                                                              |
| ---------------- | ------------------------------------------- | ------------------------------------------------------------------------------------ |
| **Topic**        | Named message channel                       | `ISSUE_OPENED`, `IMPLEMENTATION_READY`, `VALIDATION_RESULT`                          |
| **Trigger**      | Condition to wake agent                     | `{ "topic": "IMPLEMENTATION_READY", "action": "execute_task" }`                      |
| **Logic Script** | JavaScript predicate for complex conditions | `ledger.query({ topic: 'VALIDATION_RESULT' }).every(r => r.content?.data?.approved)` |
| **Hook**         | Post-task action                            | Publish message, execute command                                                     |
| **Role**         | Semantic grouping                           | `implementation`, `validator`, `orchestrator`                                        |

### System Components

| Component            | File                      | Purpose                                               |
| -------------------- | ------------------------- | ----------------------------------------------------- |
| **Orchestrator**     | @src/orchestrator.js      | Cluster lifecycle, agent spawning, GitHub integration |
| **AgentWrapper**     | @src/agent-wrapper.js     | Claude CLI wrapper, trigger eval, context building    |
| **MessageBus**       | @src/message-bus.js       | Pub/sub, WebSocket broadcast, topic routing           |
| **Ledger**           | @src/ledger.js            | SQLite append-only log, query API, crash recovery     |
| **LogicEngine**      | @src/logic-engine.js      | JavaScript sandbox for triggers, ledger/cluster APIs  |
| **IsolationManager** | @src/isolation-manager.js | Docker lifecycle for isolated execution               |
| **TUI Dashboard**    | @src/tui/                 | Interactive monitoring, real-time stats               |

### Message Flow

```
┌─────────────────────────────────────────────────────────────────────┐
│                         CLUSTER RUNTIME                              │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  Agent A                    MessageBus                    Agent B    │
│  ┌──────┐                   ┌───────┐                    ┌──────┐   │
│  │ PTY  │ ──publish()──────>│ SQLite│<──subscribeTopic()─│ PTY  │   │
│  │Claude│                   │Ledger │                    │Claude│   │
│  └──────┘                   └───────┘                    └──────┘   │
│     │                           │                           ▲       │
│     │                           ▼                           │       │
│     │                    ┌──────────┐                       │       │
│     └────────────────────│LogicEngine│───────────────────────┘       │
│                          │ Evaluate  │                               │
│                          │ Triggers  │                               │
│                          └──────────┘                               │
└─────────────────────────────────────────────────────────────────────┘
```

1. Agent A completes task → `hooks.onComplete` publishes to topic (e.g., `IMPLEMENTATION_READY`)
2. MessageBus appends to SQLite Ledger → emits `topic:IMPLEMENTATION_READY` event
3. LogicEngine evaluates all agents' triggers against this message
4. Agent B's trigger matches → AgentWrapper builds context from ledger → spawns Claude CLI

### Agent Configuration Schema

```json
{
  "id": "unique-agent-id",
  "role": "semantic-role",
  "model": "sonnet|opus|haiku",

  "triggers": [
    {
      "topic": "TOPIC_NAME",
      "action": "execute_task|stop_cluster",
      "logic": {
        "engine": "javascript",
        "script": "return true; // JS predicate with ledger/cluster APIs"
      }
    }
  ],

  "contextStrategy": {
    "sources": [
      { "topic": "ISSUE_OPENED", "limit": 1 },
      { "topic": "VALIDATION_RESULT", "since": "last_task_end", "limit": 10 }
    ],
    "maxTokens": 100000
  },

  "prompt": "Agent instructions (string or object with system/outputFormat)",

  "outputFormat": "stream-json|json",
  "jsonSchema": { "type": "object", "properties": {...} },

  "hooks": {
    "onComplete": {
      "action": "publish_message",
      "config": {
        "topic": "OUTPUT_TOPIC",
        "content": {
          "text": "{{result.summary}}",
          "data": { "approved": "{{result.approved}}" }
        }
      }
    }
  },

  "maxIterations": 30,
  "maxRetries": 1
}
```

### Logic Script API

Trigger scripts run in a sandboxed JavaScript VM with these APIs:

```javascript
// Ledger API (auto-scoped to cluster)
ledger.query({ topic: 'X', sender: 'Y', since: timestamp, limit: N }); // Query messages
ledger.findLast({ topic: 'X' }); // Get most recent message
ledger.count({ topic: 'X' }); // Count messages

// Cluster API
cluster.getAgents(); // All agents in cluster
cluster.getAgentsByRole('validator'); // Agents with specific role
cluster.getAgent('worker'); // Single agent by ID

// Helper Functions
helpers.allResponded(agents, topic, since); // Check if all agents responded
helpers.hasConsensus(topic, since); // All responses have approved=true
helpers.timeSinceLastMessage(topic); // Milliseconds since last message

// Context
(agent.id, agent.role, agent.iteration); // Current agent info
(message.topic, message.sender, message.content); // Triggering message
```

### Consensus Logic Example

```javascript
// Wait for ALL validators to approve
const validators = cluster.getAgentsByRole('validator');
const lastPush = ledger.findLast({ topic: 'IMPLEMENTATION_READY' });
const responses = ledger.query({
  topic: 'VALIDATION_RESULT',
  since: lastPush.timestamp,
});
return responses.length >= validators.length && responses.every((r) => r.content?.data?.approved);
```

### Template Variables in Hooks

Hooks support template substitution:

| Variable              | Value                            |
| --------------------- | -------------------------------- |
| `{{cluster.id}}`      | Cluster ID                       |
| `{{result.summary}}`  | Parsed summary from agent output |
| `{{result.approved}}` | Parsed approved boolean          |
| `{{result.errors}}`   | Parsed errors array              |
| `{{error.message}}`   | Error message (onError hook)     |

### Creating Custom Workflows

**Define message flow → create agents for each step → add orchestrator.**

Example: ISSUE_OPENED → Worker → IMPLEMENTATION_READY → [Validators] → VALIDATION_RESULT

- Worker: triggered by ISSUE_OPENED, publishes IMPLEMENTATION_READY
- Validators: triggered by IMPLEMENTATION_READY, publish VALIDATION_RESULT
- Orchestrator: triggered by VALIDATION_RESULT (with consensus logic), stops cluster

## Conductor System: 2D Classification → Parameterized Templates

**Conductor classifies tasks on 2 dimensions, routes to base templates with parameters.**

### Classification Dimensions

**Complexity (how hard):**

- TRIVIAL: 1 command/file, mechanical
- SIMPLE: 1 concern, straightforward
- STANDARD: multi-file, needs planning
- CRITICAL: high risk (auth, payments, security)

**TaskType (what action):**

- INQUIRY: read-only exploration ("How does X work?")
- TASK: implement new ("Add feature X")
- DEBUG: fix broken ("Why is X failing?")

### Base Templates (Parameterized)

**4 base templates replace 12 static configs:**

| Template           | Used When                      | Agents                        |
| ------------------ | ------------------------------ | ----------------------------- |
| `single-worker`    | TRIVIAL                        | 1 worker                      |
| `worker-validator` | SIMPLE INQUIRY/TASK            | worker ↔ validator loop       |
| `debug-workflow`   | SIMPLE+ DEBUG                  | investigator → fixer → tester |
| `full-workflow`    | STANDARD/CRITICAL INQUIRY/TASK | planner → worker → validators |

**Parameters injected by classification:**

- `complexity` → validator_count (0/1/2/4), max_iterations (1/3/5/7)
- `task_type` → behavior (read-only/implement/fix)
- `*_model` → haiku (TRIVIAL), sonnet (SIMPLE/STANDARD), opus (CRITICAL planner)

**Templates:** @cluster-templates/base-templates/

**Flow:**

```
COMPLEXITY × TASKTYPE
  ↓
helpers.getConfig() → { base, params }
  ↓
TemplateResolver.resolve(base, params) → agents
  ↓
Orchestrator spawns
```

### Model Selection by Complexity

| Complexity | Planner | Worker | Validators |
| ---------- | ------- | ------ | ---------- |
| TRIVIAL    | -       | haiku  | 0          |
| SIMPLE     | -       | sonnet | 1 (sonnet) |
| STANDARD   | sonnet  | sonnet | 3 (sonnet) |
| CRITICAL   | opus    | sonnet | 5 (sonnet) |

### Adversarial Tester (STANDARD/CRITICAL only)

The **adversarial-tester** is a validator with one job: **prove the implementation works by USING IT YOURSELF.**

**Core principle:** DO NOT trust existing tests. Tests can be outdated, incomplete, or passing while implementation is broken. **TESTS PASSING ≠ IMPLEMENTATION WORKS.**

The ONLY way to know if it works: **USE IT YOURSELF** (ad-hoc testing).

**The algorithm (language-agnostic):**

1. **UNDERSTAND** - Read the issue/task. What SPECIFICALLY was supposed to be built?
2. **FIGURE OUT HOW TO USE IT** - Look at the code. How do you invoke this feature?
3. **ACTUALLY USE IT** - Run the command, call the function, hit the endpoint. Did it work?
4. **TRY TO BREAK IT** - Empty input, invalid input, wrong order, call it twice
5. **VERIFY REQUIREMENTS** - For EACH requirement in the issue, show evidence (command + output)

**Existing tests are SECONDARY:**

- Tests passing does NOT mean approved
- Tests failing does NOT necessarily mean rejected (tests might be outdated)
- YOUR AD-HOC TESTING is the primary verification

**Approval criteria:**

- ✅ You PERSONALLY verified the feature works
- ✅ You have evidence (actual commands + outputs)
- ✅ No critical bugs found during ad-hoc testing

**Rejection criteria:**

- ❌ You couldn't figure out how to use it
- ❌ It doesn't do what the issue asked for
- ❌ It crashes or errors on reasonable usage
- ❌ You found critical bugs

### Workflow Patterns

| Pattern            | Flow                                                                    |
| ------------------ | ----------------------------------------------------------------------- |
| **Pipeline**       | ISSUE → Planner → Worker → Reviewer → APPROVED                          |
| **Fan-Out/Fan-In** | IMPLEMENTATION → [Validators parallel] → Consensus → COMPLETE           |
| **Rejection Loop** | Worker → Validators → (rejected) → Worker → ... → (approved) → COMPLETE |
| **Hierarchical**   | Supervisor → [Workers parallel] → Aggregator → COMPLETE                 |
| **Expert Panel**   | ISSUE → [Specialists parallel] → Aggregator → FINAL_REVIEW              |
| **Staged Gate**    | Worker → Stage1 → Stage2 → ... → COMPLETE (rejected = retry)            |

### Custom Pattern Example

**Define topic flow → map to agent configs.**

Security pipeline with mandatory approval:

- Implementer → CODE_READY
- SecurityScanner → SECURITY_RESULT
- SecurityGate → stop_cluster (if no vulnerabilities)

## Isolation Mode (Docker Container)

**Isolates workspace in fresh git clone, protects working directory. NOT about credentials (always mounted) or security sandboxing.**

**Use when:**

- Big refactors touching many files
- Risky experiments you might discard
- Long-running tasks (keep working locally)
- Parallel agents on same codebase
- PR workflows (`--pr`, `--merge` imply `--isolation`)

**Skip when:**

- Quick fixes (want immediate results)
- Debugging current state
- Interactive back-and-forth
- Read-only investigation

**Features:** Fresh git clone, Claude/AWS/kube credentials mounted, infra tools + Chromium pre-installed, auto-cleanup

## Persistence

| File                          | Content                               |
| ----------------------------- | ------------------------------------- |
| `~/.zeroshot/clusters.json`   | Cluster metadata, state, failure info |
| `~/.zeroshot/<cluster-id>.db` | SQLite message ledger                 |

**Clusters survive crashes.** Resume: `zeroshot resume <cluster-id>`

### Cluster Lifecycle

```
STOPPED → start() → INITIALIZING → (agents spawn) → RUNNING
RUNNING → stop() → STOPPING → (SIGTERM) → STOPPED
RUNNING → kill() → (SIGKILL) → KILLED
RUNNING → (idle 2min) → auto-stop
RUNNING → CLUSTER_COMPLETE message → auto-stop
RUNNING → CLUSTER_FAILED message → auto-stop
```

## Single-Agent Tasks

**For simple tasks without coordination:**

```bash
zeroshot task run "Implement X"          # Background task
zeroshot task run --output-format json   # Structured JSON
```

**Exit codes:** 0 = success, non-zero = failure
**Storage:** `~/.claude-zeroshots/`

## Development

```bash
cd zeroshot && npm link     # Install globally (once)
# Edit code → changes apply immediately (symlink)
npm test                    # Mocha tests

zeroshot --completion >> ~/.bashrc   # Shell completion
```

**Patterns:**

- Human-readable IDs: `task-swift-falcon`, `cluster-bold-panther`
- Storage: JSON + SQLite (atomic read-modify-write)
- Processes: PTY child processes, graceful SIGINT/SIGTERM

## Common Issues

| Issue             | Fix                                                |
| ----------------- | -------------------------------------------------- |
| Command not found | `npm i -g @covibes/zeroshot` or `npm link` for dev |
| Stale process     | `task kill <id>` or `zeroshot kill <id>`           |
| Orphaned logs     | `task clean -a`                                    |

## Known Limitation: Bash Subprocess Output

**Bash tool subprocess output NOT streamed in real-time**

- WHY: Claude CLI's Bash tool returns `tool_result` AFTER subprocess completes
- Symptom: 60s script → no output for 60s → all output at once
- NOT a zeroshot bug: Claude CLI architecture limitation
- Workaround: Long tasks write to file, periodically check

**What IS streamed:** Claude's text (`text_delta`), thinking, tool invocations (NOT tool results)
