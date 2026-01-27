# Zeroshot: Functional Specification

> Multi-Agent Coordination Engine for Autonomous Software Development

**Version:** 5.4.0
**Last Updated:** 2026-01-24
**Purpose:** PRD Source Document

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Core Concepts](#2-core-concepts)
3. [System Architecture](#3-system-architecture)
4. [Cluster Lifecycle](#4-cluster-lifecycle)
5. [Agent System](#5-agent-system)
6. [Template System](#6-template-system)
7. [Conductor Classification](#7-conductor-classification)
8. [Isolation Modes](#8-isolation-modes)
9. [CLI Interface](#9-cli-interface)
10. [Data Persistence](#10-data-persistence)
11. [Issue Providers](#11-issue-providers)
12. [Configuration](#12-configuration)
13. [Technical Specifications](#13-technical-specifications)

---

## 1. Executive Summary

### 1.1 What is Zeroshot?

Zeroshot is a **multi-agent coordination engine** that orchestrates AI agents (primarily Claude) to collaboratively solve software engineering tasks. It uses a **pub/sub message bus** over **SQLite persistence** to enable agents to communicate, coordinate, and execute tasks autonomously.

### 1.2 Key Value Propositions

| Capability               | Description                                                                       |
| ------------------------ | --------------------------------------------------------------------------------- |
| **Autonomous Execution** | Agents work without human intervention, making decisions autonomously             |
| **Cost Optimization**    | 2D classification routes tasks to appropriate model tiers (Haiku → Sonnet → Opus) |
| **Quality Assurance**    | Multi-validator workflows ensure implementation correctness                       |
| **Isolation**            | Git worktree and Docker modes prevent codebase pollution                          |
| **Persistence**          | Crash recovery via SQLite ledger enables resume after failures                    |
| **Platform Agnostic**    | Supports GitHub, GitLab, Azure DevOps, Jira, Gitea, and custom providers          |

### 1.3 High-Level Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                         CLI Interface                            │
│              (zeroshot run | status | logs | resume)            │
└─────────────────────────┬───────────────────────────────────────┘
                          │
┌─────────────────────────▼───────────────────────────────────────┐
│                       Orchestrator                               │
│   (Cluster lifecycle, agent management, operation handling)      │
└─────────────────────────┬───────────────────────────────────────┘
                          │
        ┌─────────────────┼─────────────────┐
        │                 │                 │
┌───────▼───────┐ ┌───────▼───────┐ ┌───────▼───────┐
│  Conductor    │ │   Workers     │ │  Validators   │
│ (Classify)    │ │ (Implement)   │ │ (Verify)      │
└───────┬───────┘ └───────┬───────┘ └───────┬───────┘
        │                 │                 │
        └─────────────────┼─────────────────┘
                          │
┌─────────────────────────▼───────────────────────────────────────┐
│                      Message Bus                                 │
│            (Pub/Sub event routing and delivery)                  │
└─────────────────────────┬───────────────────────────────────────┘
                          │
┌─────────────────────────▼───────────────────────────────────────┐
│                    SQLite Ledger                                 │
│              (Persistent message storage)                        │
└─────────────────────────────────────────────────────────────────┘
```

---

## 2. Core Concepts

### 2.1 Primitives

Zeroshot is built on four core primitives:

#### 2.1.1 Topics

Named message channels for pub/sub communication. Agents publish messages to topics and subscribe to topics via triggers.

**Standard Topics:**

| Topic                  | Publisher        | Purpose                                 |
| ---------------------- | ---------------- | --------------------------------------- |
| `ISSUE_OPENED`         | System/Conductor | Bootstrap trigger with task description |
| `PLAN_READY`           | Planner          | Execution plan for implementation       |
| `IMPLEMENTATION_READY` | Worker           | Completed implementation                |
| `VALIDATION_RESULT`    | Validator        | Approval or rejection with feedback     |
| `CLUSTER_OPERATIONS`   | Conductor        | Dynamic cluster configuration           |
| `CLUSTER_COMPLETE`     | System           | Successful completion signal            |
| `CLUSTER_FAILED`       | System           | Failure signal                          |
| `AGENT_LIFECYCLE`      | Agents           | State transitions (STARTED, COMPLETED)  |
| `AGENT_OUTPUT`         | Agents           | Streaming output during execution       |
| `TOKEN_USAGE`          | Agents           | Cost tracking per execution             |

#### 2.1.2 Triggers

Conditions that wake an agent when a matching message arrives.

```json
{
  "topic": "IMPLEMENTATION_READY",
  "action": "execute_task",
  "logic": {
    "engine": "javascript",
    "script": "return message.content.data?.canValidate === true;"
  }
}
```

**Trigger Components:**

- `topic` - Pattern to match (exact, wildcard `*`, or prefix `VALIDATION_*`)
- `action` - What to do (`execute_task` or `stop_cluster`)
- `logic` - Optional JavaScript predicate for conditional execution

#### 2.1.3 Logic Scripts

JavaScript code executed in a sandboxed VM for trigger/hook evaluation.

**Available APIs:**

```javascript
// Ledger queries (auto-scoped to cluster)
ledger.query({ topic, sender, since, limit });
ledger.findLast({ topic });
ledger.count({ topic });

// Cluster introspection
cluster.getAgents();
cluster.getAgentsByRole('validator');
cluster.getAgent(id);

// Helper functions
helpers.allResponded(agents, topic, since);
helpers.hasConsensus(topic, since);
helpers.getConfig(complexity, taskType);
```

**Security:** 1-second timeout, no fs/network/child_process access, frozen prototypes.

#### 2.1.4 Hooks

Post-execution actions that run after agent task completion.

**Hook Types:**

- `onStart` - Before task execution
- `onComplete` - After successful completion
- `onError` - After failure

**Actions:**

- `publish_message` - Publish message with template substitution or transform script

```json
{
  "onComplete": {
    "action": "publish_message",
    "config": {
      "topic": "PLAN_READY",
      "content": {
        "text": "{{result.plan}}",
        "data": { "summary": "{{result.summary}}" }
      }
    }
  }
}
```

### 2.2 Message Structure

```javascript
{
  id: "msg_a1b2c3d4",           // Auto-generated unique ID
  timestamp: 1706000000000,     // Monotonic Unix milliseconds
  cluster_id: "cluster_xyz",    // Parent cluster
  topic: "PLAN_READY",          // Message topic
  sender: "planner",            // Sending agent ID
  sender_model: "claude-sonnet-4-5",
  receiver: "broadcast",        // Target (or specific agent ID)
  content: {
    text: "Human-readable content",
    data: {                     // Structured machine-readable data
      summary: "...",
      acceptanceCriteria: [...]
    }
  },
  metadata: {                   // Optional metadata
    _republished: true
  }
}
```

---

## 3. System Architecture

### 3.1 Component Overview

| Component             | File                       | Responsibility                                   |
| --------------------- | -------------------------- | ------------------------------------------------ |
| **Orchestrator**      | `src/orchestrator.js`      | Cluster lifecycle, agent management, persistence |
| **Message Bus**       | `src/message-bus.js`       | Pub/sub event routing and delivery               |
| **Ledger**            | `src/ledger.js`            | SQLite message persistence                       |
| **Agent Wrapper**     | `src/agent-wrapper.js`     | Agent state machine and task execution           |
| **Logic Engine**      | `src/logic-engine.js`      | JavaScript sandbox for triggers/hooks            |
| **Config Router**     | `src/config-router.js`     | Complexity × TaskType routing                    |
| **Template Resolver** | `src/template-resolver.js` | Template parameterization                        |
| **Isolation Manager** | `src/isolation-manager.js` | Docker/worktree isolation                        |

### 3.2 Data Flow

```
User Input (issue URL, text, file)
    │
    ▼
┌─────────────────────────────────────────────────────────────┐
│ Orchestrator                                                 │
│ ┌─────────────┐  ┌─────────────┐  ┌─────────────────────┐  │
│ │ Issue       │  │ Template    │  │ Isolation           │  │
│ │ Provider    │  │ Resolver    │  │ Manager             │  │
│ └─────────────┘  └─────────────┘  └─────────────────────┘  │
└────────────────────────┬────────────────────────────────────┘
                         │
    ┌────────────────────┼────────────────────┐
    │                    │                    │
    ▼                    ▼                    ▼
┌────────┐          ┌────────┐          ┌────────┐
│ Agent  │◄────────►│ Agent  │◄────────►│ Agent  │
│ (Cond) │          │ (Work) │          │ (Val)  │
└────┬───┘          └────┬───┘          └────┬───┘
     │                   │                   │
     └───────────────────┼───────────────────┘
                         │
                         ▼
              ┌──────────────────┐
              │   Message Bus    │
              │ (EventEmitter)   │
              └────────┬─────────┘
                       │
                       ▼
              ┌──────────────────┐
              │  SQLite Ledger   │
              │ (WAL mode)       │
              └──────────────────┘
```

### 3.3 Agent Subsystem

The agent system is modular with specialized components:

| Component                    | File                                | Purpose             |
| ---------------------------- | ----------------------------------- | ------------------- |
| `agent-lifecycle.js`         | State machine, start/stop           | Core agent control  |
| `agent-task-executor.js`     | Task spawning, streaming, parsing   | Execution engine    |
| `agent-context-builder.js`   | Context assembly from ledger        | Prompt construction |
| `agent-hook-executor.js`     | Hook transformation and execution   | Post-task actions   |
| `agent-trigger-evaluator.js` | Trigger matching and evaluation     | Message handling    |
| `agent-config.js`            | Config validation and normalization | Configuration       |
| `agent-stuck-detector.js`    | Liveness monitoring                 | Health checks       |

---

## 4. Cluster Lifecycle

### 4.1 State Machine

```
                    ┌──────────────┐
                    │ zeroshot run │
                    └──────┬───────┘
                           │
                           ▼
                  ┌─────────────────┐
                  │  initializing   │
                  └────────┬────────┘
                           │
                   [ISSUE_OPENED published]
                           │
                           ▼
                    ┌──────────┐
           ┌────────┤  running │◄────────┐
           │        └────┬─────┘         │
           │             │               │
   [CLUSTER_COMPLETE]    │    [zeroshot resume]
   [CLUSTER_FAILED]      │               │
   [zeroshot stop]       │        ┌──────┴──────┐
           │             │        │   stopped   │
           │             │        └─────────────┘
           │    [zeroshot kill]         ▲
           │             │               │
           ▼             ▼               │
      ┌─────────┐   ┌────────┐     ┌────┴─────┐
      │stopping │──>│ killed │     │  failed  │
      └────┬────┘   └────────┘     └──────────┘
           │
           ▼
      ┌─────────┐
      │ stopped │
      └─────────┘
```

### 4.2 Cluster States

| State          | Description                     | Recoverable  |
| -------------- | ------------------------------- | ------------ |
| `initializing` | Setup in progress               | No           |
| `running`      | Normal operation                | N/A          |
| `stopping`     | Graceful shutdown               | No           |
| `stopped`      | Cleanly stopped                 | Yes (resume) |
| `killed`       | Force terminated                | No           |
| `failed`       | Task failed                     | Yes (resume) |
| `corrupted`    | 0 messages (SIGINT during init) | No           |

### 4.3 Creation Flow

**When `zeroshot run <issue>` is executed:**

1. **Initialization**
   - Generate unique cluster ID (e.g., "pensive-hippo-ab12")
   - Create SQLite database at `~/.zeroshot/<cluster_id>.db`
   - Initialize Ledger and MessageBus

2. **Input Resolution**
   - Detect issue provider from URL or git remote
   - Fetch issue content (title, body, metadata)

3. **Isolation Setup** (if requested)
   - Create git worktree (`--worktree`) or Docker container (`--docker`)
   - Mount credentials and configure environment

4. **Agent Configuration**
   - Load conductor-bootstrap template
   - Instantiate AgentWrapper for each agent
   - Inject cluster context (cwd, isolation settings)

5. **Subscription Registration** (CRITICAL: before starting agents)
   - Subscribe to `CLUSTER_COMPLETE`, `CLUSTER_FAILED`
   - Subscribe to `CLUSTER_OPERATIONS` for dynamic config
   - Subscribe to `AGENT_LIFECYCLE` for watchdog

6. **Agent Startup**
   - Start all agents (begin listening for messages)
   - Each agent subscribes to message bus

7. **Workflow Bootstrap**
   - Set cluster state to `running`
   - Publish `ISSUE_OPENED` message
   - Save cluster metadata to `~/.zeroshot/clusters.json`

### 4.4 Completion Detection

**Mechanisms:**

1. **Standard Completion Detector**
   - Waits for all validators to respond
   - Checks consensus (all approved)
   - Publishes `CLUSTER_COMPLETE`

2. **Git-Pusher Agent** (--pr mode)
   - Waits for validator approval
   - Creates PR/MR
   - Publishes `CLUSTER_COMPLETE` after PR created

3. **Max Iterations Failure**
   - Worker reaches maxIterations
   - Publishes `CLUSTER_FAILED`

4. **Conductor Watchdog**
   - Monitors conductor hook execution
   - If no `CLUSTER_OPERATIONS` within 30s → `CLUSTER_FAILED`

### 4.5 Resume Functionality

**`zeroshot resume <cluster_id> [prompt]`:**

1. Validate cluster exists and is not running
2. Restore isolation (recreate Docker container or verify worktree)
3. Restart all agents
4. Load recent context (last 50 messages)
5. Determine resume strategy:
   - **Failed cluster:** Inject error context, publish `RESUME_TASK`
   - **Clean resume:** Find last workflow trigger, resume from that point

---

## 5. Agent System

### 5.1 Agent Configuration

```javascript
{
  "id": "worker",                    // Unique identifier
  "role": "implementation",          // Role category
  "modelLevel": "level2",            // level1/level2/level3
  "prompt": {
    "system": "You are...",          // System prompt
    "initial": "...",                // First iteration
    "subsequent": "..."              // Retry iterations
  },
  "outputFormat": "json",            // json | text
  "jsonSchema": {...},               // Expected output structure
  "contextStrategy": {               // How to build context
    "sources": [
      { "topic": "ISSUE_OPENED", "limit": 1 },
      { "topic": "VALIDATION_RESULT", "since": "last_agent_start" }
    ],
    "maxTokens": 100000
  },
  "triggers": [{                     // What wakes this agent
    "topic": "PLAN_READY",
    "action": "execute_task"
  }],
  "hooks": {                         // Post-execution actions
    "onComplete": {...}
  },
  "maxIterations": 5,                // Retry limit
  "timeout": 0,                      // Task timeout (0 = none)
  "enableLivenessCheck": true,       // Stuck detection
  "staleDuration": 300000            // 5 min stale threshold
}
```

### 5.2 Agent State Machine

```
idle → evaluating_logic → building_context → executing → idle
                                                  │
                                            (on error) → failed
```

### 5.3 Agent Roles

| Role                  | Purpose                             | Typical Agents                        |
| --------------------- | ----------------------------------- | ------------------------------------- |
| `conductor`           | Task classification, config routing | junior-conductor, senior-conductor    |
| `planning`            | Strategy, investigation, planning   | planner, investigator                 |
| `implementation`      | Code execution, bug fixing          | worker, fixer                         |
| `validator`           | Verification, testing, review       | validator, tester, security-validator |
| `completion-detector` | Auto-stop when task done            | git-pusher, completion-detector       |

### 5.4 Context Building

Agents receive context assembled from ledger queries:

1. **Query ledger** for each source in `contextStrategy.sources`
2. **Merge messages** chronologically
3. **Build sections:**
   - Header (agent ID, role, iteration)
   - Instructions (system prompt, output schema)
   - Message history (formatted)
4. **Truncate** to maxTokens limit
5. **Inject warnings:**
   - No `AskUserQuestion` (autonomous execution)
   - Git restrictions (if not isolated)

### 5.5 Task Execution Flow

```
Trigger matches message
    │
    ▼
Build context from ledger
    │
    ▼
Spawn Claude task (`ct` CLI or Docker exec)
    │
    ▼
Stream output to AGENT_OUTPUT topic
    │
    ▼
Parse result (JSON extraction or fallback)
    │
    ▼
Execute hooks (publish messages)
    │
    ▼
Publish TASK_COMPLETED with token usage
```

---

## 6. Template System

### 6.1 Base Templates

Located in `cluster-templates/base-templates/`:

| Template           | Complexity        | Agents                          | Use Case                |
| ------------------ | ----------------- | ------------------------------- | ----------------------- |
| `single-worker`    | TRIVIAL           | Worker only                     | Simple edits, inquiries |
| `worker-validator` | SIMPLE            | Worker + 1 Validator            | Small features, bugs    |
| `debug-workflow`   | DEBUG             | Investigator → Fixer → Tester   | Bug fixes               |
| `full-workflow`    | STANDARD/CRITICAL | Planner → Worker → N Validators | Complex features        |

### 6.2 Template Parameterization

Templates use `{{param}}` placeholders resolved at runtime:

```json
{
  "params": {
    "worker_level": { "type": "string", "default": "level2" },
    "validator_count": { "type": "number", "default": 2 }
  },
  "agents": [
    {
      "id": "worker",
      "modelLevel": "{{worker_level}}"
    },
    {
      "id": "validator-2",
      "condition": "{{validator_count}} >= 2"
    }
  ]
}
```

### 6.3 Conditional Agents

Agents can have a `condition` field evaluated at resolution time:

```json
{
  "id": "validator-security",
  "condition": "{{validator_count}} >= 3",
  "role": "validator"
}
```

If condition evaluates to false, agent is excluded from cluster.

### 6.4 Full Workflow Details

**Agents:**

- `planner` - Creates execution plan with acceptance criteria
- `worker` - Implements the plan
- `validator-requirements` - Verifies acceptance criteria
- `validator-code` (if count ≥ 2) - Code review
- `validator-security` (if count ≥ 3) - Security audit
- `validator-tester` (if count ≥ 4) - Test execution

**Message Flow:**

```
ISSUE_OPENED → Planner
    │
    ▼
PLAN_READY → Worker
    │
    ▼
IMPLEMENTATION_READY → Validators (parallel)
    │
    ▼
VALIDATION_RESULT (from each validator)
    │
    ├── All approved → CLUSTER_COMPLETE
    │
    └── Any rejected → Worker retries (up to maxIterations)
```

### 6.5 Debug Workflow Details

**Agents:**

- `investigator` - Analyzes bug, finds root causes, scans for similar patterns
- `fixer` - Fixes all root causes, adds regression tests
- `tester` - Verifies fixes behaviorally (runs commands)

**Key Behaviors:**

- Mandatory similarity scan (grep for same bug pattern)
- Root cause mapping (each fix maps to specific cause)
- Behavioral testing (run commands, don't just read code)

---

## 7. Conductor Classification

### 7.1 Two-Tier Architecture

```
ISSUE_OPENED
    │
    ▼
Junior Conductor (level1 - cheap)
    │
    ├── CERTAIN → CLUSTER_OPERATIONS → Load template
    │
    └── UNCERTAIN → CONDUCTOR_ESCALATE
                        │
                        ▼
                 Senior Conductor (level2 - smarter)
                        │
                        └── CLUSTER_OPERATIONS → Load template
```

### 7.2 Classification Dimensions

**Complexity:**

| Level    | Description              | Validators | Model Level     |
| -------- | ------------------------ | ---------- | --------------- |
| TRIVIAL  | 1 file, mechanical       | 0          | level1 (Haiku)  |
| SIMPLE   | 1-2 files, low risk      | 1          | level2 (Sonnet) |
| STANDARD | Multi-file, user-visible | 2-3        | level2 (Sonnet) |
| CRITICAL | Auth/payments/security   | 4-5        | level3 (Opus)   |

**TaskType:**

| Type    | Description           | Preferred Template |
| ------- | --------------------- | ------------------ |
| INQUIRY | Read-only exploration | single-worker      |
| TASK    | Implement new feature | full-workflow      |
| DEBUG   | Fix broken code       | debug-workflow     |

### 7.3 Routing Logic

```javascript
function getConfig(complexity, taskType) {
  const base =
    taskType === 'DEBUG' && complexity !== 'TRIVIAL'
      ? 'debug-workflow'
      : complexity === 'TRIVIAL'
        ? 'single-worker'
        : complexity === 'SIMPLE'
          ? 'worker-validator'
          : 'full-workflow';

  return {
    base,
    params: {
      complexity,
      task_type: taskType,
      validator_count: { TRIVIAL: 0, SIMPLE: 1, STANDARD: 2, CRITICAL: 4 }[complexity],
      worker_level: complexity === 'TRIVIAL' ? 'level1' : 'level2',
      planner_level: complexity === 'CRITICAL' ? 'level3' : 'level2',
      max_tokens: { TRIVIAL: 50000, SIMPLE: 100000, STANDARD: 100000, CRITICAL: 150000 }[
        complexity
      ],
    },
  };
}
```

### 7.4 Cost Optimization

Conductor prompts explicitly bias toward STANDARD over CRITICAL to avoid false positives (CRITICAL uses Opus at $15/M tokens).

**Common False Positives (not CRITICAL):**

- Refactoring code that mentions auth (not modifying auth logic)
- Adding TypeScript types to existing structures
- Code cleanup in infrastructure files
- Tests for sensitive code (tests don't touch production)

---

## 8. Isolation Modes

### 8.1 Flag Cascade

```
--ship → --pr → --worktree (automatic escalation)
```

| Flag              | Isolation          | PR Creation | Auto-Merge |
| ----------------- | ------------------ | ----------- | ---------- |
| (none)            | None               | No          | No         |
| `--worktree`      | Git worktree       | No          | No         |
| `--docker`        | Docker container   | No          | No         |
| `--pr`            | Worktree (default) | Yes         | No         |
| `--ship`          | Worktree (default) | Yes         | Yes        |
| `--pr --docker`   | Docker             | Yes         | No         |
| `--ship --docker` | Docker             | Yes         | Yes        |

### 8.2 Worktree Isolation

**Setup:**

1. Create worktree at `/tmp/zeroshot-worktrees/<cluster_id>`
2. Create branch `zeroshot/<cluster_id>`
3. Agents execute with cwd set to worktree path

**Advantages:**

- Fast setup (<1s)
- Shares git objects with main repo
- Preserves git history

**Limitations:**

- No package manager isolation
- Shared node_modules
- Same environment as host

**Cleanup:**

- `stop()` - Preserves worktree for resume
- `kill()` - Removes worktree, preserves branch for PR inspection

### 8.3 Docker Isolation

**Setup:**

1. Create workspace at `/tmp/zeroshot-isolated/<cluster_id>`
2. Copy files (excluding .git, node_modules, build artifacts)
3. Initialize fresh git repo
4. Start container with mounted workspace and credentials
5. Auto-install npm dependencies

**Credential Mount Presets:**

| Preset      | Host Path          | Container Path     |
| ----------- | ------------------ | ------------------ |
| `gh`        | `~/.config/gh`     | `~/.config/gh`     |
| `git`       | `~/.gitconfig`     | `~/.gitconfig`     |
| `ssh`       | `~/.ssh`           | `~/.ssh`           |
| `aws`       | `~/.aws`           | `~/.aws`           |
| `azure`     | `~/.azure`         | `~/.azure`         |
| `kube`      | `~/.kube`          | `~/.kube`          |
| `terraform` | `~/.terraform.d`   | `~/.terraform.d`   |
| `gcloud`    | `~/.config/gcloud` | `~/.config/gcloud` |

**Configuration:**

```bash
# Persistent
zeroshot settings set dockerMounts '["gh","git","ssh","aws"]'

# Per-run
zeroshot run 123 --docker --mount ~/.custom:/root/.custom:ro

# Disable all
zeroshot run 123 --docker --no-mounts
```

**Environment Passthrough:**

```bash
# Syntax
VAR        # Pass if set in host
VAR_*      # Pass all matching (e.g., TF_VAR_*)
VAR=value  # Always set to value
VAR=       # Always set to empty

# Config
zeroshot settings set dockerEnvPassthrough '["NPM_TOKEN", "TF_VAR_*"]'
```

### 8.4 PR Workflow

**Agent Injection:**

1. Remove default completion-detector
2. Detect git platform from remote URL
3. Generate platform-specific git-pusher agent
4. git-pusher waits for validator approval, then:
   - Stages and commits changes
   - Pushes to origin
   - Creates PR/MR via platform CLI
   - Sets auto-merge (if `--ship`)

**Platform Support:**

| Platform     | CLI Tool   | PR Command           |
| ------------ | ---------- | -------------------- |
| GitHub       | `gh`       | `gh pr create`       |
| GitLab       | `glab`     | `glab mr create`     |
| Azure DevOps | `az repos` | `az repos pr create` |
| Gitea        | `tea`      | `tea pulls create`   |

---

## 9. CLI Interface

### 9.1 Commands

| Command                         | Description           |
| ------------------------------- | --------------------- |
| `zeroshot run <issue>`          | Start new cluster     |
| `zeroshot list`                 | List all clusters     |
| `zeroshot status <id>`          | Show cluster details  |
| `zeroshot logs <id> [-f]`       | Stream cluster logs   |
| `zeroshot resume <id> [prompt]` | Resume failed cluster |
| `zeroshot stop <id>`            | Graceful stop         |
| `zeroshot kill <id>`            | Force kill            |
| `zeroshot watch`                | TUI dashboard         |
| `zeroshot export <id>`          | Export conversation   |
| `zeroshot agents list`          | Show available agents |
| `zeroshot settings`             | View/modify settings  |

### 9.2 Run Options

| Option            | Description                                    |
| ----------------- | ---------------------------------------------- |
| `--worktree`      | Git worktree isolation                         |
| `--docker`        | Docker container isolation                     |
| `--pr`            | Create pull request (implies --worktree)       |
| `--ship`          | Full automation with auto-merge (implies --pr) |
| `-d, --daemon`    | Run in background                              |
| `--id <id>`       | Custom cluster ID                              |
| `--config <name>` | Use specific config template                   |
| `--github`        | Force GitHub provider                          |
| `--gitlab`        | Force GitLab provider                          |
| `--devops`        | Force Azure DevOps provider                    |

### 9.3 UX Modes

| Mode       | Command                | Ctrl+C Behavior              |
| ---------- | ---------------------- | ---------------------------- |
| Foreground | `zeroshot run`         | Stops cluster                |
| Daemon     | `zeroshot run -d`      | Detaches (cluster continues) |
| Attach     | `zeroshot attach <id>` | Detaches only                |

---

## 10. Data Persistence

### 10.1 Storage Locations

| Data             | Location                                   | Format |
| ---------------- | ------------------------------------------ | ------ |
| Cluster metadata | `~/.zeroshot/clusters.json`                | JSON   |
| Message ledger   | `~/.zeroshot/<cluster_id>.db`              | SQLite |
| Settings         | `~/.zeroshot/settings.json`                | JSON   |
| Terraform state  | `~/.zeroshot/terraform-state/<cluster_id>` | Files  |

### 10.2 SQLite Schema

```sql
CREATE TABLE messages (
  id TEXT PRIMARY KEY,
  timestamp INTEGER NOT NULL,
  topic TEXT NOT NULL,
  sender TEXT NOT NULL,
  receiver TEXT NOT NULL,
  content_text TEXT,
  content_data TEXT,
  metadata TEXT,
  cluster_id TEXT NOT NULL
);

CREATE INDEX idx_cluster_topic ON messages(cluster_id, topic);
CREATE INDEX idx_cluster_timestamp ON messages(cluster_id, timestamp);
```

### 10.3 Crash Recovery

1. **Cluster metadata** persisted to `clusters.json` at each state change
2. **Message ledger** uses WAL mode (survives crashes)
3. **Resume** reconstructs state from ledger
4. **Isolation** recreated from saved paths/container IDs

---

## 11. Issue Providers

### 11.1 Supported Providers

| Provider     | URL Pattern                                     | CLI Tool    |
| ------------ | ----------------------------------------------- | ----------- |
| GitHub       | `github.com/org/repo/issues/123`                | `gh`        |
| GitLab       | `gitlab.com/org/repo/-/issues/123`              | `glab`      |
| Azure DevOps | `dev.azure.com/org/project/_workitems/edit/123` | `az boards` |
| Jira         | `org.atlassian.net/browse/PROJ-123`             | N/A (API)   |
| Gitea        | `gitea.example.com/org/repo/issues/123`         | `tea`       |
| Beads        | `beads-123`                                     | `bd`        |

### 11.2 Auto-Detection

Priority order for bare numbers (`zeroshot run 123`):

1. Force flags (`--github`, `--gitlab`, `--devops`)
2. Git remote detection (parse `git remote get-url origin`)
3. Settings (`defaultIssueSource`)
4. Legacy fallback (GitHub)

### 11.3 Provider Interface

```javascript
class IssueProvider {
  static id = 'github'
  static supportsPR() { return true }

  static detectIdentifier(input, settings, gitContext) { ... }
  async fetchIssue(identifier) { ... }
  static getPRTool() { ... }
}
```

---

## 12. Configuration

### 12.1 Settings

| Setting                | Type   | Default              | Description                      |
| ---------------------- | ------ | -------------------- | -------------------------------- |
| `maxModel`             | string | `sonnet`             | Cost ceiling (haiku/sonnet/opus) |
| `defaultConfig`        | string | -                    | Default cluster config           |
| `logLevel`             | string | `info`               | Log verbosity                    |
| `defaultProvider`      | string | `claude`             | AI provider                      |
| `defaultIssueSource`   | string | -                    | Issue provider                   |
| `dockerMounts`         | array  | `['gh','git','ssh']` | Docker mount presets             |
| `dockerEnvPassthrough` | array  | `[]`                 | Env vars to pass to Docker       |
| `dockerContainerHome`  | string | `/root`              | Container home directory         |

### 12.2 Environment Variables

| Variable                 | Purpose                      |
| ------------------------ | ---------------------------- |
| `ANTHROPIC_API_KEY`      | Claude API authentication    |
| `OPENAI_API_KEY`         | OpenAI API authentication    |
| `GOOGLE_API_KEY`         | Gemini API authentication    |
| `GITHUB_TOKEN`           | GitHub API access            |
| `GITLAB_TOKEN`           | GitLab API access            |
| `AZURE_DEVOPS_PAT`       | Azure DevOps access          |
| `ZEROSHOT_DOCKER_MOUNTS` | Docker mount override (JSON) |
| `ZEROSHOT_SQLITE_*`      | SQLite tuning                |

### 12.3 Model Levels

Provider-agnostic model tiers:

| Level  | Claude | OpenAI      | Gemini |
| ------ | ------ | ----------- | ------ |
| level1 | Haiku  | GPT-4o-mini | Flash  |
| level2 | Sonnet | GPT-4o      | Pro    |
| level3 | Opus   | o1          | Ultra  |

---

## 13. Technical Specifications

### 13.1 Performance Characteristics

| Metric             | Value                             |
| ------------------ | --------------------------------- |
| Worktree setup     | <1 second                         |
| Docker setup       | ~30-60 seconds (with npm install) |
| Message throughput | ~1000 msg/sec (SQLite write)      |
| Context limit      | 100-150K tokens per agent         |
| Stale detection    | 5 minutes (default)               |

### 13.2 Concurrency Model

- **Message Bus:** Synchronous EventEmitter (no message replay)
- **Ledger:** SQLite with WAL (concurrent reads, serial writes)
- **Agents:** Single-threaded per agent, parallel across agents
- **Locking:** `proper-lockfile` for cluster registry

### 13.3 Security Considerations

| Area             | Protection                             |
| ---------------- | -------------------------------------- |
| Logic scripts    | VM sandbox, 1s timeout, no fs/net      |
| Docker isolation | Credential mounts read-only by default |
| Git operations   | Restricted unless isolated             |
| AskUserQuestion  | Blocked in autonomous mode             |
| Secrets          | Never committed (.env, credentials)    |

### 13.4 Error Handling

| Error Type      | Behavior                         |
| --------------- | -------------------------------- |
| API rate limit  | Exponential backoff retry        |
| Network failure | Retry with timeout               |
| Agent timeout   | Stuck detection → CLUSTER_FAILED |
| Hook failure    | Watchdog → CLUSTER_FAILED        |
| Parse failure   | Fallback text extraction         |

### 13.5 File Organization

```
zeroshot/
├── cli/
│   └── index.js              # CLI entry point
├── src/
│   ├── orchestrator.js       # Core cluster management
│   ├── message-bus.js        # Pub/sub layer
│   ├── ledger.js             # SQLite persistence
│   ├── agent-wrapper.js      # Agent state machine
│   ├── logic-engine.js       # JS sandbox
│   ├── config-router.js      # Classification routing
│   ├── template-resolver.js  # Template parameterization
│   ├── isolation-manager.js  # Docker/worktree
│   ├── agent/                # Agent subsystem
│   ├── issue-providers/      # GitHub, GitLab, etc.
│   └── providers/            # Claude, OpenAI, Gemini
├── cluster-templates/
│   ├── conductor-bootstrap.json
│   └── base-templates/
│       ├── single-worker.json
│       ├── worker-validator.json
│       ├── debug-workflow.json
│       └── full-workflow.json
└── lib/
    ├── settings.js           # User settings
    ├── docker-config.js      # Docker mounts
    └── git-remote-utils.js   # Git detection
```

---

## Appendix A: Message Flow Examples

### A.1 Simple Task (TRIVIAL)

```
1. User: zeroshot run 123
2. System → ISSUE_OPENED
3. junior-conductor → CLUSTER_OPERATIONS { base: 'single-worker' }
4. worker spawns, receives republished ISSUE_OPENED
5. worker → IMPLEMENTATION_READY
6. completion-detector → CLUSTER_COMPLETE
7. Cluster stops
```

### A.2 Feature Implementation (STANDARD)

```
1. User: zeroshot run 456 --pr
2. System → ISSUE_OPENED
3. junior-conductor → CLUSTER_OPERATIONS { base: 'full-workflow', validator_count: 2 }
4. planner → PLAN_READY
5. worker → IMPLEMENTATION_READY
6. validator-1, validator-2 → VALIDATION_RESULT (parallel)
7. [If rejected, worker retries]
8. git-pusher → push, create PR → CLUSTER_COMPLETE
9. Cluster stops
```

### A.3 Bug Fix (DEBUG)

```
1. User: zeroshot run 789
2. System → ISSUE_OPENED
3. junior-conductor → CLUSTER_OPERATIONS { base: 'debug-workflow' }
4. investigator → INVESTIGATION_COMPLETE (root causes + similar patterns)
5. fixer → IMPLEMENTATION_READY (all fixes + tests)
6. tester → VALIDATION_RESULT (behavioral verification)
7. [If rejected, fixer retries]
8. completion-detector → CLUSTER_COMPLETE
9. Cluster stops
```

---

## Appendix B: Glossary

| Term          | Definition                                          |
| ------------- | --------------------------------------------------- |
| **Cluster**   | A group of agents working together on a task        |
| **Agent**     | An AI instance with specific role and triggers      |
| **Topic**     | Named message channel                               |
| **Trigger**   | Condition that wakes an agent                       |
| **Hook**      | Post-execution action                               |
| **Conductor** | Agent that classifies tasks and routes to templates |
| **Ledger**    | SQLite database storing all messages                |
| **Worktree**  | Lightweight git branch isolation                    |
| **Isolation** | Separation from main working directory              |

---

## Appendix C: Changelog

| Version | Date    | Changes                                     |
| ------- | ------- | ------------------------------------------- |
| 5.4.0   | 2026-01 | Gitea support, OAuth token auth             |
| 5.3.0   | 2026-01 | Beads integration, improved stuck detection |
| 5.2.0   | 2025-12 | Two-tier conductor, cost optimization       |
| 5.0.0   | 2025-11 | Initial public release                      |
