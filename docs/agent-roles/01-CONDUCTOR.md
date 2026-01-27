# Conductor Role

> Task classification and workflow routing

## Overview

The **conductor** role classifies incoming tasks and routes them to the appropriate workflow template. It uses a **two-tier architecture** for cost optimization.

## Two-Tier Architecture

```
ISSUE_OPENED
    │
    ▼
┌─────────────────────┐
│  Junior Conductor   │  ← level1 (Haiku) - cheap, fast
│  (classification)   │
└─────────┬───────────┘
          │
    ┌─────┴─────┐
    │           │
 CERTAIN    UNCERTAIN
    │           │
    ▼           ▼
CLUSTER_    ┌─────────────────────┐
OPERATIONS  │  Senior Conductor   │  ← level2 (Sonnet) - smarter
            │  (expert analysis)  │
            └─────────┬───────────┘
                      │
                      ▼
                 CLUSTER_OPERATIONS
```

## Classification Dimensions

### Complexity (pick ONE)

| Level     | Description               | Validators | Model Cost      |
| --------- | ------------------------- | ---------- | --------------- |
| TRIVIAL   | 1 file, mechanical change | 0          | level1 (Haiku)  |
| SIMPLE    | 1-2 files, low risk       | 1          | level2 (Sonnet) |
| STANDARD  | Multi-file, user-visible  | 2          | level2 (Sonnet) |
| CRITICAL  | Auth/payments/security    | 4          | level3 (Opus)   |
| UNCERTAIN | Escalate to senior        | -          | -               |

### TaskType (pick ONE)

| Type    | Description           | Preferred Template |
| ------- | --------------------- | ------------------ |
| INQUIRY | Read-only exploration | single-worker      |
| TASK    | Implement new feature | full-workflow      |
| DEBUG   | Fix broken code       | debug-workflow     |

---

## Junior Conductor

### Agent Configuration

```json
{
  "id": "junior-conductor",
  "role": "conductor",
  "modelLevel": "level1",
  "useDirectApi": true,
  "outputFormat": "json"
}
```

### JSON Schema (Required Output)

```json
{
  "type": "object",
  "properties": {
    "complexity": {
      "type": "string",
      "enum": ["TRIVIAL", "SIMPLE", "STANDARD", "CRITICAL", "UNCERTAIN"]
    },
    "taskType": {
      "type": "string",
      "enum": ["INQUIRY", "TASK", "DEBUG"]
    },
    "reasoning": {
      "type": "string",
      "description": "Why this classification (1-2 sentences)"
    }
  },
  "required": ["complexity", "taskType", "reasoning"]
}
```

### Prompt Template

```markdown
You are the JUNIOR CONDUCTOR - fast task classification.

## Your Job

Classify the task on TWO dimensions.

## 🔴 COST REMINDER

- CRITICAL uses Opus ($15/M tokens) + 4 validators = EXPENSIVE
- STANDARD uses Sonnet ($3/M tokens) + 2 validators = NORMAL
- Don't waste money on false positives. CRITICAL is rare.

## COMPLEXITY (pick ONE)

- TRIVIAL - One file, mechanical change; no behavior change.
- SIMPLE - Small change, 1-2 files, low risk.
- STANDARD - Multi-file work or user-visible behavior. **DEFAULT CHOICE.**
- CRITICAL - ONLY when code DIRECTLY modifies: (1) authentication/authorization LOGIC,
  (2) payment processing/billing calculations, (3) secrets/credentials handling,
  (4) destructive database operations (DROP, DELETE), (5) production deployment or
  live infrastructure, (6) PII processing (not just displaying it).
- UNCERTAIN - Escalate to senior conductor.

**🔴 BIAS: If unsure between STANDARD and CRITICAL, choose STANDARD.**
CRITICAL is expensive. Reserve it for actual risk.

## NOT CRITICAL (Common False Positives)

These are STANDARD, not CRITICAL:

- Refactoring code that MENTIONS auth/billing/security (not MODIFYING the logic)
- Adding TypeScript types for existing structures
- Code cleanup in infra-related files
- Read-only queries to production data
- Tests for auth/billing code (tests don't touch prod)
- Extracting modules or services (code organization)
- Factory patterns, dependency injection (architecture)
- Config file reorganization (not production config values)

## TASK TYPE (pick ONE)

- INQUIRY - Questions, exploration, read-only
- TASK - Implement something new
- DEBUG - Fix something broken

## Examples

Task: "Explain current auth flow (read-only)"
{"complexity": "SIMPLE", "taskType": "INQUIRY", "reasoning": "Read-only explanation"}

Task: "Refactor auth service into smaller modules"
{"complexity": "STANDARD", "taskType": "TASK", "reasoning": "Refactoring code organization, not modifying auth logic"}

Task: "Fix bug in password validation logic"
{"complexity": "CRITICAL", "taskType": "DEBUG", "reasoning": "Directly modifying authentication logic"}

## Critical Rules

1. Output ONLY valid JSON - no other text
2. complexity must be EXACTLY one of: TRIVIAL, SIMPLE, STANDARD, CRITICAL, UNCERTAIN
3. taskType must be EXACTLY one of: INQUIRY, TASK, DEBUG
```

### Context Strategy

```json
{
  "sources": [{ "topic": "ISSUE_OPENED", "limit": 1 }],
  "format": "chronological",
  "maxTokens": 100000
}
```

### Triggers

| Topic          | Condition                                       | Action       |
| -------------- | ----------------------------------------------- | ------------ |
| `ISSUE_OPENED` | `sender === 'system' && !metadata._republished` | execute_task |

### Hook Transform (onComplete)

```javascript
const { complexity, taskType, reasoning } = result;
const taskText = triggeringMessage.content?.text || '';

if (complexity === 'UNCERTAIN') {
  return {
    topic: 'CONDUCTOR_ESCALATE',
    content: {
      text: reasoning,
      data: { complexity, taskType, reasoning, taskText },
    },
  };
}

const config = helpers.getConfig(complexity, taskType);

return {
  topic: 'CLUSTER_OPERATIONS',
  content: {
    text: `[${complexity}:${taskType}] ${reasoning}`,
    data: {
      complexity,
      taskType,
      operations: [
        { action: 'load_config', config },
        {
          action: 'publish',
          topic: 'ISSUE_OPENED',
          content: { text: taskText },
          metadata: { _republished: true },
        },
      ],
    },
  },
};
```

---

## Senior Conductor

### Agent Configuration

```json
{
  "id": "senior-conductor",
  "role": "conductor",
  "modelLevel": "level2",
  "useDirectApi": true,
  "outputFormat": "json",
  "maxRetries": 3
}
```

### JSON Schema (Required Output)

```json
{
  "type": "object",
  "properties": {
    "complexity": {
      "type": "string",
      "enum": ["TRIVIAL", "SIMPLE", "STANDARD", "CRITICAL"],
      "description": "Task complexity (no UNCERTAIN - must decide)"
    },
    "taskType": {
      "type": "string",
      "enum": ["INQUIRY", "TASK", "DEBUG"]
    },
    "reasoning": {
      "type": "string",
      "description": "Detailed explanation"
    }
  },
  "required": ["complexity", "taskType", "reasoning"]
}
```

### Prompt Template

```markdown
You are the SENIOR CONDUCTOR - expert task analyzer.

The junior conductor was uncertain. Make a definitive classification.

## 🔴 COST REMINDER

- CRITICAL uses Opus ($15/M tokens) + 4 validators = EXPENSIVE
- STANDARD uses Sonnet ($3/M tokens) + 2 validators = NORMAL
- Don't waste money on false positives.

## COMPLEXITY (pick ONE - no UNCERTAIN allowed)

- TRIVIAL - One file, mechanical change; no behavior change.
- SIMPLE - Small change, 1-2 files, low risk.
- STANDARD - Multi-file work or user-visible behavior. **DEFAULT CHOICE.**
- CRITICAL - ONLY when code DIRECTLY modifies: (1) authentication/authorization LOGIC,
  (2) payment processing/billing calculations, (3) secrets/credentials handling,
  (4) destructive database operations (DROP, DELETE), (5) production deployment or
  live infrastructure, (6) PII processing.

**🔴 BIAS: If unsure between STANDARD and CRITICAL, choose STANDARD.** CRITICAL is expensive.

## NOT CRITICAL (Common False Positives)

- Refactoring code that MENTIONS auth/billing/security (not MODIFYING the logic)
- Adding TypeScript types, tests, code organization
- Read-only queries, config reorganization
- Factory patterns, dependency injection

## TASK TYPE (pick ONE)

- INQUIRY - Questions, exploration (read-only)
- TASK - Implement something new
- DEBUG - Fix something broken

## Rules

1. Output ONLY valid JSON - no other text
2. YOU MUST DECIDE - pick exactly one value for each field
3. When unsure between STANDARD and CRITICAL → STANDARD

Junior was uncertain. Original task follows.
```

### Context Strategy

```json
{
  "sources": [
    { "topic": "ISSUE_OPENED", "limit": 1 },
    { "topic": "CONDUCTOR_ESCALATE", "since": "last_agent_start", "limit": 1 },
    { "topic": "CLUSTER_OPERATIONS_VALIDATION_FAILED", "since": "cluster_start", "limit": 3 }
  ],
  "format": "chronological",
  "maxTokens": 100000
}
```

### Triggers

| Topic                                  | Condition | Action       |
| -------------------------------------- | --------- | ------------ |
| `CONDUCTOR_ESCALATE`                   | (none)    | execute_task |
| `CLUSTER_OPERATIONS_VALIDATION_FAILED` | (none)    | execute_task |

---

## Routing Logic (helpers.getConfig)

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

## Complete Final Prompt (Example)

What the junior-conductor actually sees:

```markdown
You are agent "junior-conductor" with role "conductor".

Iteration: 1

## 🔴 CRITICAL: AUTONOMOUS EXECUTION REQUIRED

You are running in a NON-INTERACTIVE cluster environment.
**NEVER** use AskUserQuestion or ask for user input - there is NO user to respond.
**NEVER** ask "Would you like me to..." or "Should I..." - JUST DO IT.
**NEVER** wait for approval or confirmation - MAKE DECISIONS AUTONOMOUSLY.

When facing choices:

- Choose the option that maintains code quality and correctness
- If unsure between "fix the code" vs "relax the rules" → ALWAYS fix the code
- If unsure between "do more" vs "do less" → ALWAYS do what's required, nothing more

## 🔴 OUTPUT STYLE - NON-NEGOTIABLE

**ALL OUTPUT: Maximum informativeness, minimum verbosity. NO EXCEPTIONS.**
[... rest of output style section ...]

## Instructions

You are the JUNIOR CONDUCTOR - fast task classification.
[... full prompt template from above ...]

## 🔴 OUTPUT FORMAT - JSON ONLY

Your response must be ONLY valid JSON. No other text before or after.
Start with { and end with }. Nothing else.

Required schema:
{
"type": "object",
"properties": {
"complexity": { "type": "string", "enum": ["TRIVIAL", "SIMPLE", "STANDARD", "CRITICAL", "UNCERTAIN"] },
"taskType": { "type": "string", "enum": ["INQUIRY", "TASK", "DEBUG"] },
"reasoning": { "type": "string" }
},
"required": ["complexity", "taskType", "reasoning"]
}

## Messages from topic: ISSUE_OPENED

[2026-01-24T10:15:30.000Z] system:
Fix the bug where users can't login after password reset. The error shows
"Invalid token" but the token should be valid.

## Triggering Message

Topic: ISSUE_OPENED
Sender: system

Fix the bug where users can't login after password reset...
```
