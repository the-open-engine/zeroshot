# Prompt Construction Pipeline

> How agent prompts are dynamically assembled at runtime

## Overview

Agent prompts are NOT static strings. They are **dynamically constructed** at runtime by the `agent-context-builder.js` module. Understanding this pipeline is essential for understanding what each agent actually "sees".

## The Construction Pipeline

```
┌───────────────────────────────────────────────────────────────────┐
│                     FINAL AGENT PROMPT                            │
├───────────────────────────────────────────────────────────────────┤
│                                                                   │
│  1. HEADER CONTEXT (auto-injected)                               │
│     ├── Agent ID and Role                                        │
│     ├── Iteration Number                                         │
│     ├── 🔴 AUTONOMOUS EXECUTION section                          │
│     ├── 🔴 OUTPUT STYLE section                                  │
│     └── 🚫 GIT OPERATIONS section (if NOT isolated)             │
│                                                                   │
│  2. INSTRUCTIONS (from agent config)                             │
│     └── The `prompt.system` or `prompt.initial/subsequent`       │
│                                                                   │
│  3. OUTPUT SCHEMA (if JSON output)                               │
│     ├── jsonSchema definition                                    │
│     └── Auto-generated example                                   │
│                                                                   │
│  4. CONTEXT SOURCES (from ledger queries)                        │
│     ├── ## Messages from topic: ISSUE_OPENED                    │
│     ├── ## Messages from topic: PLAN_READY                      │
│     └── ## Messages from topic: VALIDATION_RESULT               │
│                                                                   │
│  5. VALIDATOR SKIP SECTION (validators only)                     │
│     └── ⚠️ Permanently Unverifiable Criteria                    │
│                                                                   │
│  6. TRIGGERING MESSAGE                                           │
│     └── The message that woke this agent                         │
│                                                                   │
└───────────────────────────────────────────────────────────────────┘
```

## Section Details

### 1. Header Context (Auto-Injected)

Every agent receives this header automatically:

```markdown
You are agent "worker" with role "implementation".

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

This applies to EVERYTHING you output:

- Text responses
- JSON schema values
- Reasoning fields
- Summary fields
- ALL string values in structured output

Rules:

- Progress: "Reading auth.ts" NOT "I will now read the auth.ts file..."
- Tool calls: NO preamble. Call immediately.
- Schema strings: Dense facts. No filler. No fluff.
- Errors: DETAILED (stack traces, repro). NEVER compress errors.
- FORBIDDEN: "I'll help...", "Let me...", "I'm going to...", "Sure!", "Great!", "Certainly!"

Every token costs money. Waste nothing.

## 🚫 GIT OPERATIONS - FORBIDDEN

(Only injected if NOT running in isolated mode)

NEVER commit, push, or create PRs. You only modify files.
The git-pusher agent handles ALL git operations AFTER validators approve.

- ❌ NEVER run: git add, git commit, git push, gh pr create
- ❌ NEVER suggest committing changes
- ✅ Only modify files and publish your completion message when done
```

### 2. Instructions Section

The agent's actual prompt from config:

```markdown
## Instructions

[Content from agent.prompt.system OR agent.prompt.initial/subsequent]
```

For agents with iteration-based prompts:

- **Iteration 1:** Uses `prompt.initial`
- **Iteration 2+:** Uses `prompt.subsequent` (stronger, addresses rejection)

### 3. JSON Schema Section (If outputFormat="json")

````markdown
## 🔴 OUTPUT FORMAT - JSON ONLY

Your response must be ONLY valid JSON. No other text before or after.
Start with { and end with }. Nothing else.

Required schema:

```json
{
  "type": "object",
  "properties": {
    "approved": { "type": "boolean" },
    "summary": { "type": "string" },
    "errors": { "type": "array", "items": { "type": "string" } }
  },
  "required": ["approved", "summary"]
}
```
````

Example output:

```json
{
  "approved": true,
  "summary": "summary value",
  "errors": []
}
```

CRITICAL RULES:

- Output ONLY the JSON object - no explanation, no thinking, no preamble
- Use EXACTLY the enum values specified (case-sensitive)
- Include ALL required fields

````

### 4. Context Sources Section

Messages from ledger, formatted chronologically:

```markdown
## Messages from topic: ISSUE_OPENED

[2026-01-24T10:15:30.000Z] system:
Fix the authentication bug where users can't login after password reset.

Data: {
  "issueNumber": 123,
  "issueTitle": "Login fails after password reset"
}

## Messages from topic: PLAN_READY

[2026-01-24T10:16:45.000Z] planner:
1. Investigate auth flow in auth-service.ts
2. Check password reset token validation
3. Fix token verification logic
4. Add regression test

Data: {
  "summary": "Fix token validation in auth flow",
  "acceptanceCriteria": [...]
}
````

### 5. Validator Skip Section (Validators Only)

Prevents validators from re-attempting permanently impossible checks:

```markdown
## ⚠️ Permanently Unverifiable Criteria (SKIP THESE)

The following criteria have PERMANENT environmental limitations (missing tools, no access).
These limitations have not changed. Do NOT re-attempt verification.
Mark these as CANNOT_VALIDATE again with the same reason.

- **AC3**: kubectl not installed in environment
- **AC5**: No SSH access to production server
```

### 6. Triggering Message Section

The message that woke this agent:

```markdown
## Triggering Message

Topic: PLAN_READY
Sender: planner

1. Investigate auth flow in auth-service.ts
2. Check password reset token validation
   ...
```

## Context Strategy Configuration

Each agent defines which messages it receives:

```json
{
  "contextStrategy": {
    "sources": [
      { "topic": "ISSUE_OPENED", "limit": 1 },
      { "topic": "PLAN_READY", "limit": 1 },
      { "topic": "VALIDATION_RESULT", "since": "last_task_end", "limit": 10 }
    ],
    "format": "chronological",
    "maxTokens": 100000
  }
}
```

### Timestamp Anchors

| Anchor             | Meaning                                     |
| ------------------ | ------------------------------------------- |
| `cluster_start`    | When cluster was created                    |
| `last_task_end`    | When this agent last finished executing     |
| `last_agent_start` | When this agent's current iteration started |

## Token Limits and Truncation

1. **Strategy limit:** `maxTokens` in contextStrategy (default 100,000)
2. **Defensive limit:** 500,000 chars (~125k tokens) - prevents overflow errors

When truncation is needed:

1. Header is always preserved
2. ISSUE_OPENED is always preserved
3. Triggering message is always preserved
4. Middle context (older messages) is truncated from the START

## Template Parameter Substitution

Before context building, template parameters are resolved:

```json
// Template config
{ "modelLevel": "{{worker_level}}", "maxTokens": "{{max_tokens}}" }

// Resolved with params: { worker_level: "level2", max_tokens: 100000 }
{ "modelLevel": "level2", "maxTokens": 100000 }
```

Conditional sections in prompts use Handlebars-like syntax:

```
{{#if task_type == 'DEBUG'}}
This is a debug task - find root cause.
{{/if}}
```

## Isolation Context

When running with `--worktree` or `--docker`:

- **Git Operations section is NOT injected** (agents can commit)
- `isIsolated` flag is true in header builder

## Summary: What Each Agent Actually Sees

| Section                      | Source                                  | Can Override?       |
| ---------------------------- | --------------------------------------- | ------------------- |
| Header (ID, role, iteration) | Auto-generated                          | No                  |
| Autonomous execution rules   | Auto-injected                           | No                  |
| Output style rules           | Auto-injected                           | No                  |
| Git restrictions             | Auto-injected (unless isolated)         | Via isolation flags |
| Instructions                 | `agent.prompt.*` config                 | Yes                 |
| JSON schema                  | `agent.jsonSchema` config               | Yes                 |
| Context messages             | Ledger queries via `contextStrategy`    | Yes                 |
| Validator skip list          | Auto-built from CANNOT_VALIDATE results | No                  |
| Triggering message           | The message that matched trigger        | No                  |
