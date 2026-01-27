# Agent Roles Documentation

> Complete reference for all zeroshot agent roles and their prompt templates

## Quick Reference

| Role                                   | Purpose                                    | Templates                       |
| -------------------------------------- | ------------------------------------------ | ------------------------------- |
| [Conductor](01-CONDUCTOR.md)           | Task classification and routing            | conductor-bootstrap             |
| [Planner](02-PLANNER.md)               | Strategic planning and acceptance criteria | full-workflow                   |
| [Worker](03-WORKER.md)                 | Code implementation and execution          | all templates                   |
| [Validators](04-VALIDATORS.md)         | Quality verification and approval          | worker-validator, full-workflow |
| [Debug Workflow](05-DEBUG-WORKFLOW.md) | Investigator → Fixer → Tester              | debug-workflow                  |
| [Git Pusher](06-GIT-PUSHER.md)         | PR/MR creation and merging                 | injected with --pr/--ship       |

## Before Reading These

**Start with [00-PROMPT-CONSTRUCTION.md](00-PROMPT-CONSTRUCTION.md)** to understand how agent prompts are dynamically assembled from:

1. Auto-injected header (ID, role, iteration)
2. Autonomous execution rules
3. Output style rules
4. Git restrictions (unless isolated)
5. Agent-specific instructions (from config)
6. JSON schema (if applicable)
7. Ledger context messages
8. Triggering message

## Workflow Templates

### Single Worker (TRIVIAL)

```
ISSUE_OPENED → Worker → CLUSTER_COMPLETE
```

### Worker + Validator (SIMPLE)

```
ISSUE_OPENED → Worker ←→ Validator → CLUSTER_COMPLETE
                    ↑__________|
                     (retry loop)
```

### Full Workflow (STANDARD/CRITICAL)

```
ISSUE_OPENED → Planner → Worker ←→ Validators → CLUSTER_COMPLETE
                              ↑_________|
                               (retry loop)
```

### Debug Workflow (DEBUG)

```
ISSUE_OPENED → Investigator → Fixer ←→ Tester → CLUSTER_COMPLETE
                                   ↑______|
                                    (retry loop)
```

### With PR Mode (--pr or --ship)

```
... → Validators → Git-Pusher → PR_CREATED → CLUSTER_COMPLETE
```

## Role Categories

### Classification

- **Conductor** - Routes tasks to appropriate templates

### Planning

- **Planner** - Creates implementation plans
- **Investigator** - Investigates bugs, finds root causes

### Implementation

- **Worker** - Executes plans, writes code
- **Fixer** - Applies bug fixes

### Validation

- **Validator** - General verification (SIMPLE)
- **Validator-Requirements** - Checks acceptance criteria
- **Validator-Code** - Code review
- **Validator-Security** - Security audit
- **Validator-Tester** - Test execution
- **Tester** - Behavioral testing for debug workflow

### Completion

- **Git-Pusher** - Creates and merges PRs/MRs
- **Completion-Detector** - Signals task completion (default)

## Document Structure

Each role document contains:

1. **Overview** - Purpose and when used
2. **Agent Configuration** - JSON config structure
3. **JSON Schema** - Required output format (if applicable)
4. **Prompt Template** - The full prompt the agent receives
5. **Context Strategy** - What messages from ledger are included
6. **Triggers** - What messages wake this agent
7. **Hooks** - What happens after execution
8. **Workflow Position** - Where in the workflow this agent sits
9. **Key Behaviors** - Important behavioral rules

## Model Levels

| Level  | Model  | Cost    | Used For                         |
| ------ | ------ | ------- | -------------------------------- |
| level1 | Haiku  | $0.25/M | Trivial tasks, junior conductor  |
| level2 | Sonnet | $3/M    | Standard work, most agents       |
| level3 | Opus   | $15/M   | Critical tasks, complex planning |

## Key Patterns

### Iteration-Based Prompts

Workers have different prompts for:

- **Initial** (iteration 1) - Execute the plan
- **Subsequent** (iteration 2+) - Fix rejection issues

### Self-Continuation

Workers can set `canValidate: false` to continue working without triggering validators, publishing to `WORKER_PROGRESS` instead of `IMPLEMENTATION_READY`.

### Evidence-Based Approval

Validators must provide evidence (command, exitCode, output) for their decisions. The git-pusher only triggers when evidence is real.

### CANNOT_VALIDATE Handling

When verification is impossible (missing tools, no access), validators mark criteria as `CANNOT_VALIDATE`. This is tracked and subsequent validators skip re-attempting these checks.

## Common Prompt Sections

All agents receive these auto-injected sections:

```markdown
## 🔴 CRITICAL: AUTONOMOUS EXECUTION REQUIRED

- NEVER use AskUserQuestion
- NEVER ask for confirmation
- Make decisions autonomously

## 🔴 OUTPUT STYLE - NON-NEGOTIABLE

- Maximum informativeness, minimum verbosity
- FORBIDDEN: "I'll help...", "Let me...", "Sure!"

## 🚫 GIT OPERATIONS - FORBIDDEN (unless isolated)

- NEVER commit, push, or create PRs
- Only modify files
```

Key Insight: Prompt Construction Pipeline
The most important document is 00-PROMPT-CONSTRUCTION.md.
It explains that agent prompts are NOT static - they're dynamically assembled at runtime from:

┌───────────────────────────────────────────┐
│ FINAL AGENT PROMPT │
├───────────────────────────────────────────┤
│ 1. Header (ID, role, iteration) │ ← Auto-injected
│ 2. 🔴 AUTONOMOUS EXECUTION rules │ ← Auto-injected
│ 3. 🔴 OUTPUT STYLE rules │ ← Auto-injected
│ 4. 🚫 GIT OPERATIONS restrictions │ ← Auto-injected (unless isolated)
│ 5. ## Instructions │ ← From agent.prompt.system
│ 6. 🔴 OUTPUT FORMAT (JSON schema) │ ← From agent.jsonSchema
│ 7. ## Messages from topic: X │ ← Ledger queries via contextStrategy
│ 8. ⚠️ Unverifiable Criteria (validators) │ ← Auto-built from CANNOT_VALIDATE
│ 9. ## Triggering Message │ ← The message that woke this agent
└───────────────────────────────────────────┘

Each role document includes:

The full prompt template (what's in the config)
The JSON schema for required output
The context strategy (what ledger messages are included)
The triggers and hooks
Example final prompts showing what agents actually see

Combined with ZEROSHOT-FUNCTIONAL-SPECIFICATION.md, you now have comprehensive PRD source material covering both the system architecture and the agent prompt engineering.
