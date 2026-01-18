## EXECUTING DELEGATED TASKS

⚠️ SUB-AGENT LIMITS (CRITICAL - prevents context explosion):

- Maximum 3 parallel sub-agents at once
- If phase has more tasks, batch them into groups of 3
- Prioritize by dependency order, then complexity

If PLAN_READY contains a 'delegation' field in its data, you MUST use parallel sub-agents:

1. Parse delegation.phases and delegation.tasks from the plan data
2. For each phase in order:
   a. Find all tasks for this phase (matching taskIds)
   b. Split into batches of MAX 3 tasks each
   c. For each batch:
   - Spawn sub-agents using Task tool (run_in_background: true)
   - Use the model specified in each task (haiku/sonnet/opus)
   - Wait for batch to complete using TaskOutput with block: true
   - SUMMARIZE each result (see OUTPUT HANDLING below)
   - Only proceed to next batch after current batch completes
3. After ALL phases complete, verify changes work together
4. Do NOT commit until all sub-agents finish

Example Task tool call for each delegated task:

```
Task tool with:
  subagent_type: 'general-purpose'
  model: [task.model from delegation]
  prompt: '[task.description]. Files: [task.scope]. Do NOT commit.'
  run_in_background: true
```

## SUB-AGENT OUTPUT HANDLING (CRITICAL - prevents context bloat)

When TaskOutput returns a sub-agent result, SUMMARIZE immediately:

- Extract ONLY: success/failure, files modified, key outcomes
- Discard: full file contents, verbose logs, intermediate steps
- Keep as: "Task [id] completed: [2-3 sentence summary]"

Example: "Task fix-auth completed: Fixed JWT validation in auth.ts, added null check. Tests pass."

DO NOT accumulate full sub-agent output - this causes context explosion.

If NO delegation field, implement directly as normal.

## 🚀 LARGE TASKS - USE SUB-AGENTS (DEBUG MODE)

If task affects >10 files OR >50 errors, DO NOT fix manually. Use the Task tool to spawn parallel sub-agents:

1. **Analyze scope first** - Count files/errors, group by directory or error type
2. **Spawn sub-agents** - One per group, run in parallel
3. **Choose model wisely:**
   - **haiku**: Mechanical fixes (unused vars, missing imports, simple type annotations)
   - **sonnet**: Complex fixes (refactoring, logic changes, architectural decisions)
4. **Aggregate results** - Wait for all sub-agents, verify combined fix

Example Task tool usage:

```
Task(prompt="Fix all unused variable warnings in src/components/. Remove genuinely unused variables, prefix intentional ones appropriately for the language.", model="haiku")
```

DO NOT waste iterations doing manual work that sub-agents can parallelize.
