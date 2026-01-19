# zeroshot CLI

[![CI](https://github.com/covibes/zeroshot/actions/workflows/ci.yml/badge.svg)](https://github.com/covibes/zeroshot/actions/workflows/ci.yml)
[![npm version](https://img.shields.io/npm/v/@covibes/zeroshot.svg)](https://www.npmjs.com/package/@covibes/zeroshot)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Node 18+](https://img.shields.io/badge/node-18%2B-brightgreen.svg)](https://nodejs.org/)
![Platform: Linux | macOS](https://img.shields.io/badge/platform-Linux%20%7C%20macOS-blue.svg)

Zeroshot is a multi-agent coordination engine for Claude Code. It runs a planner, worker, and validators in isolated contexts, iterating until validators approve or reject with concrete issues.

## Quick Start

```bash
npm install -g @covibes/zeroshot
zeroshot run 123                    # GitHub issue number
```

Or describe the task inline:

```bash
zeroshot run "Add optimistic locking with automatic retry: when updating a user,
retry with exponential backoff up to 3 times, merge non-conflicting field changes,
and surface conflicts with details. Handle the ABA problem where version goes A->B->A."
```

## Why Multiple Agents?

Single-agent sessions degrade. Context gets buried under thousands of tokens. The model optimizes for "done" over "correct."

Zeroshot fixes this with isolated agents that check each other's work. Validators can't lie about code they didn't write. Fail the check? Fix and retry until it actually works.

## Demo

<p align="center">
  <img src="./docs/assets/zeroshot-demo.gif" alt="Demo" width="700">
  <br>
  <em>Sped up 100x. 90-minute run, 5 iterations until validators approved.</em>
</p>

In this example the validators rejected the first implementation for concurrency edge cases and retry timing issues. The worker iterated until the validators could reproduce correct behavior under load.

## What Makes It Different

- **Blind validation** - Validators never see the worker's context or code history
- **Repeatable workflows** - Task complexity determines agent count and model selection
- **Accept/reject loop** - Rejections include actionable findings, not vague complaints
- **Crash recovery** - All state persisted to SQLite; resume anytime
- **Isolation modes** - None, git worktree, or Docker container
- **Cost control** - Model ceilings prevent runaway API spend

## When to Use Zeroshot

Zeroshot performs best when tasks have clear acceptance criteria.

| Scenario                                        | Use | Why                       |
| ----------------------------------------------- | --- | ------------------------- |
| Add rate limiting (sliding window, per-IP, 429) | Yes | Clear requirements        |
| Refactor auth to JWT                            | Yes | Defined end state         |
| Fix login bug                                   | Yes | Success is measurable     |
| Fix 2410 lint violations                        | Yes | Clear completion criteria |
| Make the app faster                             | No  | Needs exploration first   |
| Improve the codebase                            | No  | No acceptance criteria    |
| Figure out flaky tests                          | No  | Exploratory               |

Rule of thumb: if you cannot describe what "done" means, validators cannot verify it.

<p align="center">
  <a href="https://discord.gg/PdZ3UEXB"><img src="https://img.shields.io/badge/Discord-Join_the_community-5865F2?logo=discord&logoColor=white" alt="Discord"></a>
</p>

## Install and Requirements

**Platforms**: Linux, macOS (Windows WSL not yet supported)

```bash
npm install -g @covibes/zeroshot
```

**Requires**: Node 18+, [Claude Code CLI](https://claude.com/product/claude-code), [GitHub CLI](https://cli.github.com/)

```bash
npm i -g @anthropic-ai/claude-code && claude auth login
gh auth login
```

## Command Overview

```bash
# Run
zeroshot run 123
zeroshot run "Add dark mode"

# Isolation
zeroshot run 123 --worktree       # git worktree
zeroshot run 123 --docker         # container

# Automation (--ship implies --pr implies --worktree)
zeroshot run 123 --pr             # worktree + create PR
zeroshot run 123 --ship           # PR + auto-merge on approval

# Background mode
zeroshot run 123 -d
zeroshot run 123 --ship -d

# Pass extra flags to Claude CLI
zeroshot task run "prompt" --cli-args "--chrome"     # browser control
zeroshot task run "prompt" --cli-args "--verbose"    # verbose mode

# Control
zeroshot list
zeroshot status <id>
zeroshot logs <id> -f
zeroshot resume <id>
zeroshot stop <id>
zeroshot kill <id>
zeroshot watch

# Agent library
zeroshot agents list
zeroshot agents show <name>

# Maintenance
zeroshot clean
zeroshot purge
```

## How It Works

Zeroshot is a message-driven coordination layer with smart defaults.

- The conductor classifies tasks by complexity and type.
- A workflow template selects agents and validators.
- Agents publish results to a SQLite ledger.
- Validators approve or reject with specific findings.
- Rejections route back to the worker for fixes.

### Complexity Model

| Task                   | Complexity | Agents | Validators                                        |
| ---------------------- | ---------- | ------ | ------------------------------------------------- |
| Fix typo in README     | TRIVIAL    | 1      | None                                              |
| Add dark mode toggle   | SIMPLE     | 2      | Generic validator                                 |
| Refactor auth system   | STANDARD   | 4      | Requirements, code                                |
| Implement payment flow | CRITICAL   | 7      | Requirements, code, security, tester, adversarial |

### Model Selection by Complexity

| Complexity | Planner | Worker | Validators |
| ---------- | ------- | ------ | ---------- |
| TRIVIAL    | -       | haiku  | -          |
| SIMPLE     | -       | sonnet | 1 (sonnet) |
| STANDARD   | sonnet  | sonnet | 2 (sonnet) |
| CRITICAL   | opus    | sonnet | 5 (sonnet) |

Set model ceiling: `zeroshot settings set maxModel sonnet`.

<details>
<summary><strong>Custom Workflows (Framework Mode)</strong></summary>

Zeroshot is message-driven, so you can define any agent topology.

- Expert panels: parallel specialists -> aggregator -> decision
- Staged gates: sequential validators, each with veto power
- Hierarchical: supervisor dynamically spawns workers
- Dynamic: conductor adds agents mid-execution

**Coordination primitives:**

- Message bus (pub/sub topics)
- Triggers (wake agents on conditions)
- Ledger (SQLite, crash recovery)
- Dynamic spawning (CLUSTER_OPERATIONS)

#### Creating Custom Clusters with Claude Code

Start Claude Code and describe your cluster:

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

Built-in validation checks for missing triggers, deadlocks, and invalid type wiring before running.

See [CLAUDE.md](./CLAUDE.md) for the cluster schema and examples.

</details>

## Crash Recovery

All state is persisted in the SQLite ledger. You can resume at any time:

```bash
zeroshot resume cluster-bold-panther
```

## Isolation Modes

### Git Worktree (Default for --pr/--ship)

```bash
zeroshot run 123 --worktree
```

Lightweight isolation using git worktree. Creates a separate working directory with its own branch. Auto-enabled with `--pr` and `--ship`.

### Docker Container

```bash
zeroshot run 123 --docker
```

Full isolation in a fresh container. Your workspace stays untouched. Useful for risky experiments or parallel runs.

### When to Use Which

| Scenario                             | Recommended            |
| ------------------------------------ | ---------------------- |
| Quick task, review changes yourself  | No isolation (default) |
| PR workflow, code review             | `--worktree` or `--pr` |
| Risky experiment, might break things | `--docker`             |
| Running multiple tasks in parallel   | `--docker`             |
| Full automation, no review needed    | `--ship`               |

**Default behavior:** Agents modify files only; they do not commit or push unless using an isolation mode that explicitly allows it.

<details>
<summary><strong>Docker Credential Mounts</strong></summary>

When using `--docker`, zeroshot mounts credential directories so agents can access tools like AWS, Azure, and kubectl.

**Default mounts**: `gh`, `git`, `ssh` (GitHub CLI, git config, SSH keys)

**Available presets**: `gh`, `git`, `ssh`, `aws`, `azure`, `kube`, `terraform`, `gcloud`

```bash
# Configure via settings (persistent)
zeroshot settings set dockerMounts '["gh", "git", "ssh", "aws", "azure"]'

# View current config
zeroshot settings get dockerMounts

# Per-run override
zeroshot run 123 --docker --mount ~/.aws:/root/.aws:ro

# Disable all mounts
zeroshot run 123 --docker --no-mounts

# CI: env var override
ZEROSHOT_DOCKER_MOUNTS='["aws","azure"]' zeroshot run 123 --docker
```

**Custom mounts** (mix presets with explicit paths):

```bash
zeroshot settings set dockerMounts '[
  "gh",
  "git",
  {"host": "~/.myconfig", "container": "$HOME/.myconfig", "readonly": true}
]'
```

**Container home**: Presets use `$HOME` placeholder. Default: `/root`. Override with:

```bash
zeroshot settings set dockerContainerHome '/home/node'
# Or per-run:
zeroshot run 123 --docker --container-home /home/node
```

**Env var passthrough**: Presets auto-pass related env vars (for example, `aws` -> `AWS_REGION`, `AWS_PROFILE`). Add custom:

```bash
zeroshot settings set dockerEnvPassthrough '["MY_API_KEY", "TF_VAR_*"]'
```

</details>

## Resources

- [CLAUDE.md](./CLAUDE.md) - Architecture, cluster config schema, agent primitives
- [Discord](https://discord.gg/PdZ3UEXB) - Support and community
- `zeroshot export <id>` - Export conversation to markdown
- `sqlite3 ~/.zeroshot/*.db` - Direct ledger access for debugging

<details>
<summary><strong>CLI Passthrough (--cli-args)</strong></summary>

The `--cli-args` option passes extra flags directly to the underlying Claude CLI. This is useful for enabling features like browser control.

```bash
# Enable browser control with --chrome
zeroshot task run "Check the Storybook for visual bugs" --cli-args "--chrome"

# Multiple flags
zeroshot task run "Debug the UI" --cli-args "--chrome --verbose"
```

**Common flags to pass:**
- `--chrome` - Enable browser control for UI/Storybook tasks
- `--verbose` - Increase Claude CLI verbosity
- `--debug` - Enable debug mode

Note: Flags are passed to Claude CLI, not to zeroshot itself.

</details>

<details>
<summary><strong>Troubleshooting</strong></summary>

| Issue                         | Fix                                                       |
| ----------------------------- | --------------------------------------------------------- |
| `claude: command not found`   | `npm i -g @anthropic-ai/claude-code && claude auth login` |
| `gh: command not found`       | [Install GitHub CLI](https://cli.github.com/)             |
| `--docker` fails              | Docker must be running: `docker ps` to verify             |
| Cluster stuck                 | `zeroshot resume <id>` to continue                        |
| Agent keeps failing           | Check `zeroshot logs <id>` for actual error               |
| `zeroshot: command not found` | `npm install -g @covibes/zeroshot`                        |

</details>

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup and guidelines.

Please read [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) before participating.

For security issues, see [SECURITY.md](SECURITY.md).

---

MIT - [Covibes](https://github.com/covibes)

Built on [Claude Code](https://claude.com/product/claude-code) by Anthropic.
