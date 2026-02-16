# Local Customisations (on top of upstream v5.4)

These are our additions to zeroshot. Kept separate from README.md and CLAUDE.md to minimise merge pain when new releases land.

## Review Workflows

Multi-agent design review system. Analysts examine artifacts from independent perspectives, a synthesiser resolves disagreements through adversarial iteration, and a report is written to disk.

### Tiers

| Tier   | Config          | Analysts                 | Validators | Max Iters | Analyst Level | Max Tokens | Use Case         |
| ------ | --------------- | ------------------------ | ---------- | --------- | ------------- | ---------- | ---------------- |
| Trace  | `review-trace`  | 2 core                   | 1          | 3         | level2        | 100k       | Quick scan       |
| Vector | `review-vector` | 3-4 (core + conditional) | 2-3        | 4         | level2        | 150k       | Standard review  |
| Axiom  | `review-axiom`  | 5-8 (all perspectives)   | 2-3        | 5         | level3        | 150k       | Maximum scrutiny |

**Note:** Fixed-tier routers use hardcoded boolean params (has_test_content, is_chain, is_sensitive). Content-aware perspective activation requires `review-conductor`.

### Running

```bash
# Fixed tier (pass-through router, no classification)
zeroshot run "Review these requirements" --config review-trace
zeroshot run requirements.md --config review-vector
zeroshot run "Review auth design + AC + tests" --config review-axiom

# Auto-classify tier (conductor picks trace/vector/axiom)
zeroshot run "Review this" --config review-conductor
```

Or via the `zs` shell alias (defined in `~/.bash_aliases`):

```bash
zs trace "Review these requirements"
zs vector requirements.md
zs axiom "Review auth design + AC + tests"
```

### How It Works

1. **Router** — pass-through or conductor classification
2. **Analysts** — subagent perspectives via Task tool
3. **Validators** — adversarial challenge/defend iterations with analyst
4. **Synthesiser** — compiles confirmed/contested/withdrawn findings into final report
5. **Report writer** — execute_system_command writes markdown to CWD

### Files

| File                                                    | Purpose                                                                  |
| ------------------------------------------------------- | ------------------------------------------------------------------------ |
| `cluster-templates/review-trace.json`                   | Fixed trace-tier router config                                           |
| `cluster-templates/review-vector.json`                  | Fixed vector-tier router config                                          |
| `cluster-templates/review-axiom.json`                   | Fixed axiom-tier router config                                           |
| `cluster-templates/review-conductor.json`               | Auto-classifying conductor config                                        |
| `cluster-templates/base-templates/review-workflow.json` | Parameterised base template (analysts, synthesiser, validator, reporter) |
| `scripts/write-review-report.js`                        | Formats SYNTHESIS_COMPLETE data as markdown report                       |

## Custom Cluster Templates

Two upstream features we added to support the review workflow:

### `execute_system_command` trigger action

Runs a shell command when a trigger fires. Message content is piped to stdin as JSON. Environment includes `CLUSTER_ID` and `ZEROSHOT_ROOT`.

```json
{
  "triggers": [
    {
      "topic": "SYNTHESIS_COMPLETE",
      "action": "execute_system_command",
      "config": {
        "command": "node $ZEROSHOT_ROOT/scripts/write-review-report.js",
        "stopClusterAfter": true,
        "timeout": 15000
      }
    }
  ]
}
```

Optional `onSuccess`/`onFailure` fields route the outcome to custom topics instead of the defaults (`CLUSTER_FAILED` on error, idle on success). Agent state is set to `idle` (not `failed`) when `onFailure` is configured, allowing re-trigger loops. Output is truncated to 5 000 chars.

```json
{
  "config": {
    "command": "node $ZEROSHOT_ROOT/scripts/quality-gate-runner.js",
    "timeout": 120000,
    "onSuccess": { "topic": "QUALITY_GATE_PASSED" },
    "onFailure": { "topic": "QUALITY_GATE_FAILED" }
  }
}
```

Implementation: `src/agent/agent-lifecycle.js:316`

### Parameterised templates

`TemplateResolver` substitutes `{{param}}` placeholders in base templates. Configs reference a base + params:

```json
{
  "action": "load_config",
  "config": {
    "base": "review-workflow",
    "params": {
      "tier": "vector",
      "analyst_level": "level2",
      "validator_count": 2,
      "max_iterations": 4
    }
  }
}
```

Conditional agents use a `"condition"` field — included only if the param evaluates truthy. Unresolved `{{param}}` placeholders fail hard. Pure placeholder values (e.g., `"{{max_tokens}}"`) preserve the original JS type — numbers stay numbers, booleans stay booleans.

Implementation: `src/template-resolver.js`

## Subagent Tracking

Live display of Claude Code subagents (spawned via Task tool) in the StatusFooter.

```
│ ● analyst [executing]  cpu 45%  mem 312M                                │
│    ├─ ● Search codebase for auth patterns                               │
│    └─ ● Analyze error handling                                          │
│ ● synthesiser [executing]  cpu 30%  mem 280M                            │
```

### How It Works

1. `buildSpawnEnv()` sets `ZEROSHOT_TRACK_SUBAGENTS=1` and `ZEROSHOT_SUBAGENT_EVENTS_FILE=<path>` for every agent
2. A Claude hook (`hooks/track-subagents.py`) writes JSONL start/stop events when the Task tool is invoked
3. `SubagentTracker` polls JSONL files every 1s, reads only new bytes (offset tracking)
4. `StatusFooter` renders active subagents as tree-prefixed rows under their parent agent

### Files

| File                               | Purpose                                        |
| ---------------------------------- | ---------------------------------------------- |
| `src/subagent-tracker.js`          | JSONL event reader with offset-based polling   |
| `src/status-footer.js`             | Renders subagent tree rows (lines ~750-770)    |
| `src/agent/agent-task-executor.js` | Sets env vars in `buildSpawnEnv()` (line ~679) |
| `src/orchestrator.js`              | Cleans up temp files on stop/kill              |

## Quality Gate

Zero-cost automated checks (lint, typecheck, tests) inserted between worker completion and validator start. Catches basic failures before spending API credits on validators.

### Message Flow

```
Worker done → IMPLEMENTATION_READY → quality-gate agent (execute_system_command)
  ├─ pass (or no .zeroshot-quality file) → QUALITY_GATE_PASSED → Validators trigger
  └─ fail → QUALITY_GATE_FAILED (stdout/stderr) → Worker re-triggers, fixes, loops
```

When `quality_gate=false` (or the quality-gate agent is absent), validators trigger directly on `IMPLEMENTATION_READY` — existing behaviour preserved.

### `.zeroshot-quality` convention

A one-liner in the project root containing the quality check command:

```
npm run lint && npm run typecheck && npm test
```

If missing: auto-pass with warning. Generated once per project via `scripts/zeroshot-init.sh`.

### Setup

```bash
# AI-assisted (uses claude/codex/gemini CLI to analyse the project)
scripts/zeroshot-init.sh /path/to/repo

# Manual
echo 'npm run lint && npm test' > .zeroshot-quality
```

The init script falls back to heuristic detection (package.json scripts, Cargo.toml, go.mod, pyproject.toml, etc.) when no AI CLI is available. Multi-ecosystem projects (e.g. Laravel + Vite, Tauri) detect both backends.

### Template param

Both `worker-validator` and `full-workflow` templates accept `quality_gate` (boolean, default: `true`). Disable with:

```json
{ "params": { "quality_gate": false } }
```

### Files

| File                                                     | Purpose                                              |
| -------------------------------------------------------- | ---------------------------------------------------- |
| `scripts/quality-gate-runner.js`                         | Reads `.zeroshot-quality`, runs command, JSON output |
| `scripts/zeroshot-init.sh`                               | One-time setup, generates `.zeroshot-quality`        |
| `cluster-templates/base-templates/worker-validator.json` | quality-gate agent + dual validator triggers         |
| `cluster-templates/base-templates/full-workflow.json`    | Same for STANDARD/CRITICAL templates                 |
| `src/agent/agent-lifecycle.js`                           | `onSuccess`/`onFailure` in execute_system_command    |
| `src/config-validator.js`                                | Validates onSuccess/onFailure topic fields           |
| `tests/quality-gate.test.js`                             | 17 tests covering all paths                          |

## Model Auto-Upgrade

Models below the configured `minLevel`/`minModel` floor are silently upgraded instead of crashing. Previously a model below the floor would throw — now it logs a warning and bumps to the minimum.

Implementation: `src/agent-wrapper.js`, `src/providers/base-provider.js`, `lib/settings.js`

## Deferred / Known Issues

- **#1B** — `context-pack-builder.js` coerces numeric values to strings (upstream, untouched)
- **#11** — Max iterations CLUSTER_FAILED race: `handleMaxIterations` stop() could kill synthesis on same tick. Works in practice but needs deeper investigation.
- **#14** — `_evaluateCondition` uses raw `params` instead of `paramsWithDefaults` (`template-resolver.js:56`)
