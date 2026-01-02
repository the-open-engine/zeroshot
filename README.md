# zeroshot CLI

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Node 18+](https://img.shields.io/badge/node-18%2B-brightgreen.svg)](https://nodejs.org/)
[![Platform: Linux | macOS](https://img.shields.io/badge/platform-Linux%20%7C%20macOS-blue.svg)]()

> **2024** was the year of LLMs. **2025** was the year of agents. **2026** is the year of agent clusters.

**Multi-agent coding CLI built on Claude Code.**

You know the problem. Your AI agent:

- Says "tests pass" (never ran them)
- Says "done!" (nothing works)
- Implements 60% of what you asked
- Ignores your coding guidelines
- Introduces antipatterns like a junior dev
- Gets sloppy on long tasks

**AI is extremely capable. But not when one agent does everything in one session.**

Context degrades. Attention drifts. Shortcuts get taken.

Zeroshot fixes this with **multiple isolated agents** that check each other's work. The validator didn't write the code, so it can't lie about tests. Fail? Fix and retry until it works.

```bash
zeroshot 123
```

Point at a GitHub issue, walk away, come back to working code.

### Demo

```bash
zeroshot "Add rate limiting middleware: sliding window algorithm (not fixed window),
per-IP tracking with in-memory store and automatic TTL cleanup to prevent memory leaks,
configurable limits per endpoint. Return 429 Too Many Requests with Retry-After header
(seconds until reset) and X-RateLimit-Remaining header on ALL responses.
Must handle both IPv4 and IPv6, normalizing IPv6 to consistent format."
```

<p align="center">
  <img src="./docs/assets/zeroshot-demo.gif" alt="Demo" width="700">
  <br>
  <em>Sped up — original recording: 32 minutes</em>
</p>

---

## Install

**Platforms**: Linux, macOS

```bash
npm install -g @covibes/zeroshot
```

**Requires**: Node 18+, [Claude Code CLI](https://claude.com/product/claude-code), [GitHub CLI](https://cli.github.com/)

```bash
npm i -g @anthropic-ai/claude-code && claude auth login
gh auth login
```

---

## Commands

```bash
zeroshot run 123               # Run on GitHub issue
zeroshot run "Add dark mode"   # Run from description

# Automation levels (cascading: --ship → --pr → --worktree)
zeroshot run 123 --docker      # Docker isolation (full container)
zeroshot run 123 --worktree    # Git worktree isolation (lightweight)
zeroshot run 123 --pr          # Worktree + PR (human reviews)
zeroshot run 123 --ship        # Worktree + PR + auto-merge (full automation)

# Background mode
zeroshot run 123 -d            # Detached/daemon
zeroshot run 123 --ship -d     # Full automation, background

# Control
zeroshot list                  # See all running (--json for scripting)
zeroshot status <id>           # Cluster status (--json for scripting)
zeroshot logs <id> -f          # Follow output
zeroshot resume <id>           # Continue after crash
zeroshot kill <id>             # Stop
zeroshot watch                 # TUI dashboard

# Agent library
zeroshot agents list           # View available agents
zeroshot agents show <name>    # Agent details

# Maintenance
zeroshot clean                 # Remove old records
zeroshot purge                 # NUCLEAR: kill all + delete all
```

---

## FAQ

**Q: Why Claude-only?**

Claude Code is the most capable agentic coding tool available. We wrap it directly - same tools, same reliability, no custom implementations to break.

**Q: Why do single-agent coding sessions get sloppy?**

Three failure modes compound when one agent does everything in one session:

- **Context Dilution**: Your initial guidelines compete with thousands of tokens of code, errors, and edits. Instructions from 50 messages ago get buried.
- **Success Bias**: LLMs optimize for "Task Complete" - even if that means skipping steps to get there.
- **Error Snowball**: When fixing mistakes repeatedly, the context fills with broken code. The model starts copying its own bad patterns.

Zeroshot fixes this with **isolated agents** where validators check work they didn't write - no self-grading, no shortcuts.

**Q: Can I customize the team?**

Yes, see CLAUDE.md. But most people never need to.

**Q: Why does the CLI appear frozen?**

Zeroshot agents use strict JSON schema outputs to ensure reliable parsing and hook execution. This is incompatible with live streaming - agents can't stream partial JSON.

During heavy tasks (large refactors, complex analysis), the CLI may appear frozen for several minutes while the agent works. This is normal - the agent is actively running, just not streaming output.

**Q: Why is it called "zeroshot"?**

In machine learning, "zero-shot" means solving tasks the model has never seen before - using only the task description, no prior examples needed.

Same idea here: give zeroshot a well-defined task, get back a result. No examples. No iterative feedback. No hand-holding.

The multi-agent architecture handles planning, implementation, and validation internally. You provide a clear problem statement. Zeroshot handles the rest.

---

## How It Works

Zeroshot is a **multi-agent coordination framework** with smart defaults.

### Zero Config

```bash
zeroshot 123  # Analyzes task → picks team → done
```

The conductor classifies your task (complexity × type) and routes to a pre-built workflow.

### Default Workflows (Out of the Box)

```
                                ┌─────────────────┐
                                │      TASK       │
                                └────────┬────────┘
                                         │
                                         ▼
                ┌────────────────────────────────────────────┐
                │                 CONDUCTOR                  │
                │     Complexity × TaskType → Workflow       │
                └────────────────────────┬───────────────────┘
                                         │
           ┌─────────────────────────────┼─────────────────────────────┐
           │                             │                             │
           ▼                             ▼                             ▼
     ┌───────────┐                ┌───────────┐                ┌───────────┐
     │  TRIVIAL  │                │  SIMPLE   │                │ STANDARD+ │
     │  1 agent  │──────────▶     │  worker   │                │ planner   │
     │  (haiku)  │  COMPLETE      │ + 1 valid.│                │ + worker  │
     │ no valid. │                └─────┬─────┘                │ + 3-5 val.│
     └───────────┘                      │                      └─────┬─────┘
                                        ▼                            │
                                 ┌─────────────┐                     ▼
                             ┌──▶│   WORKER    │             ┌─────────────┐
                             │   └──────┬──────┘             │   PLANNER   │
                             │          │                    └──────┬──────┘
                             │          ▼                           │
                             │   ┌─────────────────────┐            ▼
                             │   │ ✓ validator         │     ┌─────────────┐
                             │   │   (generic check)   │ ┌──▶│   WORKER    │
                             │   └──────────┬──────────┘ │   └──────┬──────┘
                             │       REJECT │ ALL OK     │          │
                             └──────────────┘     │      │          ▼
                                                  │      │   ┌──────────────────────┐
                                                  │      │   │ ✓ requirements       │
                                                  │      │   │ ✓ code (STANDARD+)   │
                                                  │      │   │ ✓ security (CRIT)    │
                                                  │      │   │ ✓ tester (CRIT)      │
                                                  │      │   │ ✓ adversarial        │
                                                  │      │   │   (curl + browser)   │
                                                  │      │   └──────────┬───────────┘
                                                  │      │       REJECT │ ALL OK
                                                  │      └──────────────┘     │
                                                  ▼                           ▼
     ┌─────────────────────────────────────────────────────────────────────────────┐
     │                                COMPLETE                                     │
     └─────────────────────────────────────────────────────────────────────────────┘
```

These are **templates**. The conductor picks based on what you're building.

| Task                   | Complexity | Agents | Validators                                        |
| ---------------------- | ---------- | ------ | ------------------------------------------------- |
| Fix typo in README     | TRIVIAL    | 1      | None                                              |
| Add dark mode toggle   | SIMPLE     | 2      | generic validator                                 |
| Refactor auth system   | STANDARD   | 5      | requirements, code, adversarial                   |
| Implement payment flow | CRITICAL   | 7      | requirements, code, security, tester, adversarial |

## End-to-End Flow

**This is how zeroshot processes a task from start to finish:**

```
                              ╔═════════════════════════════════════════════════════╗
                              ║            ZEROSHOT ORCHESTRATION ENGINE            ║
                              ╚═════════════════════════════════════════════════════╝

                                              ┌─────────────────┐
                                              │   "Add auth     │
                                              │   to the API"   │
                                              └────────┬────────┘
                                                       │
                                                       ▼
┌──────────────────────────────────────────────────────────────────────────────────────────────┐
│                              CONDUCTOR (2D Classification)                                    │
│  ┌─────────────────────────────────────────────────────────────────────────────────────────┐ │
│  │  Junior (Haiku)                                     Senior (Sonnet)                     │ │
│  │  ─────────────                                      ───────────────                     │ │
│  │  Fast classification on 2 dimensions:        ───▶   Handles UNCERTAIN cases             │ │
│  │  • Complexity: TRIVIAL | SIMPLE | STANDARD   (if    with deeper analysis                │ │
│  │  • TaskType: INQUIRY | TASK | DEBUG          Junior                                     │ │
│  │                                              unsure)                                    │ │
│  └─────────────────────────────────────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────────────────────────────────────┘
                                                       │
                                                       │ Classification: STANDARD × TASK
                                                       ▼
                              ┌─────────────────────────────────────────┐
                              │            CONFIG ROUTER                │
                              │  ─────────────────────────────────────  │
                              │  TRIVIAL        → single-worker         │
                              │  SIMPLE         → worker-validator      │
                              │  DEBUG (non-trivial) → debug-workflow   │
                              │  STANDARD/CRITICAL  → full-workflow  ◀──│
                              └─────────────────────────────────────────┘
                                                       │
                                                       │ Spawns full-workflow agents
                                                       ▼
┌──────────────────────────────────────────────────────────────────────────────────────────────┐
│                                    FULL WORKFLOW                                             │
│  ┌─────────────────────────────────────────────────────────────────────────────────────────┐ │
│  │                                                                                         │ │
│  │   ┌──────────────┐                                                                      │ │
│  │   │   PLANNER    │  Creates implementation plan                                         │ │
│  │   │ (opus/sonnet)│  • Analyzes requirements                                             │ │
│  │   └──────┬───────┘  • Identifies files to change                                        │ │
│  │          │          • Breaks into actionable steps                                      │ │
│  │          │ PLAN_READY                                                                   │ │
│  │          ▼                                                                              │ │
│  │   ┌──────────────┐                                                                      │ │
│  │   │    WORKER    │◀─────────────────────────────────────────────┐                       │ │
│  │   │   (sonnet)   │  Implements the plan                         │                       │ │
│  │   └──────┬───────┘  • Writes/modifies code                      │                       │ │
│  │          │          • Handles rejections                        │                       │ │
│  │          │ IMPLEMENTATION_READY                                 │                       │ │
│  │          ▼                                                      │                       │ │
│  │   ┌─────────────────────────────────────────────────────┐       │                       │ │
│  │   │              VALIDATORS (parallel)                  │       │                       │ │
│  │   │                                                     │       │                       │ │
│  │   │  ┌────────────┐ ┌────────────┐ ┌─────────────────┐  │       │ REJECTED              │ │
│  │   │  │Requirements│ │Code Review │ │  Adversarial    │  │       │                       │ │
│  │   │  │  Validator │ │  (reviewer)│ │    Tester       │  │───────┘                       │ │
│  │   │  │ (validator)│ │            │ │ EXECUTES tests  │  │                               │ │
│  │   │  └────────────┘ └────────────┘ └─────────────────┘  │                               │ │
│  │   │                                                     │                               │ │
│  │   └──────────────────────┬──────────────────────────────┘                               │ │
│  │                          │                                                              │ │
│  │                          │ ALL APPROVED                                                 │ │
│  │                          ▼                                                              │ │
│  │                   ┌──────────────┐                                                      │ │
│  │                   │   COMPLETE   │                                                      │ │
│  │                   │  ──────────  │                                                      │ │
│  │                   │  PR Created  │  (with --pr flag)                                    │ │
│  │                   │  Auto-merged │  (with --merge flag)                                 │ │
│  │                   └──────────────┘                                                      │ │
│  │                                                                                         │ │
│  └─────────────────────────────────────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────────────────────────────────────┘
```

### Model Selection by Complexity

| Complexity | Planner | Worker | Validators |
| ---------- | ------- | ------ | ---------- |
| TRIVIAL    | -       | haiku  | 0          |
| SIMPLE     | -       | sonnet | 1 (sonnet) |
| STANDARD   | sonnet  | sonnet | 3 (sonnet) |
| CRITICAL   | opus    | sonnet | 5 (sonnet) |

---

### Custom Workflows (Framework Mode)

Zeroshot is **message-driven** - define any agent topology:

- **Expert panels**: Parallel specialists → aggregator → decision
- **Staged gates**: Sequential validators, each with veto power
- **Hierarchical**: Supervisor dynamically spawns workers
- **Dynamic**: Conductor adds agents mid-execution

**Coordination primitives:**

- Message bus (pub/sub topics)
- Triggers (wake agents on conditions)
- Ledger (SQLite, crash recovery)
- Dynamic spawning (CLUSTER_OPERATIONS)

#### Creating Custom Clusters with Claude Code

**The easiest way to create a custom cluster: just ask Claude Code.**

```bash
# In your zeroshot repo
claude
```

**Example prompt:**
```
Create a zeroshot cluster config for security-critical features:

1. Implementation agent (sonnet) implements the feature
2. FOUR parallel validators:
   - Security validator: OWASP checks, SQL injection, XSS, CSRF
   - Performance validator: No N+1 queries, proper indexing
   - Privacy validator: GDPR compliance, data minimization
   - Code reviewer: General code quality

3. ALL validators must approve before merge
4. If ANY validator rejects, implementation agent fixes and resubmits
5. Use opus for security validator (highest stakes)

Look at cluster-templates/base-templates/full-workflow.json
and create a similar cluster. Save to cluster-templates/security-review.json
```

Claude Code will read existing templates, create valid JSON config, and iterate until it works.

**Built-in validation catches failures before running:**
- Never start (no bootstrap trigger)
- Never complete (no path to completion)
- Loop infinitely (circular dependencies)
- Deadlock (impossible consensus)
- Type mismatches (boolean → string in JSON)

See [CLAUDE.md](./CLAUDE.md) for cluster config schema and examples.

You don't configure defaults. But you **can** when needed.

---

## Crash Recovery

Everything saves to SQLite. If your 2-hour run crashes at 1:59:

```bash
zeroshot resume cluster-bold-panther
# Continues from exact point
```

---

## Isolation Modes

### Git Worktree (Default for --pr/--ship)

```bash
zeroshot 123 --worktree
```

Lightweight isolation using git worktree. Creates a separate working directory with its own branch. Fast (<1s setup), no Docker required. Auto-enabled with `--pr` and `--ship`.

### Docker Container

```bash
zeroshot 123 --docker
```

Full isolation in a fresh container. Your workspace stays untouched. Good for risky experiments or parallel agents.

---

## More

- **Debug**: `sqlite3 ~/.zeroshot/cluster-abc.db "SELECT * FROM messages;"`
- **Export**: `zeroshot export <id> --format markdown`
- **Architecture**: See [CLAUDE.md](./CLAUDE.md)

---

## Troubleshooting

| Issue                         | Fix                                                                  |
| ----------------------------- | -------------------------------------------------------------------- |
| `claude: command not found`   | `npm i -g @anthropic-ai/claude-code && claude auth login`            |
| `gh: command not found`       | [Install GitHub CLI](https://cli.github.com/)                        |
| CLI frozen for minutes        | Normal - agents use JSON schema output, can't stream partial results |
| `--docker` fails              | Docker must be running: `docker ps` to verify                        |
| Cluster stuck                 | `zeroshot resume <id>` to continue with guidance                     |
| Agent keeps failing           | Check `zeroshot logs <id>` for actual error                          |
| `zeroshot: command not found` | `npm install -g @covibes/zeroshot`                                   |

---

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup and guidelines.

Please read our [Code of Conduct](CODE_OF_CONDUCT.md) before participating.

For security issues, see [SECURITY.md](SECURITY.md).

---

MIT — [Covibes](https://github.com/covibes)

Built on [Claude Code](https://claude.com/product/claude-code) by Anthropic.


