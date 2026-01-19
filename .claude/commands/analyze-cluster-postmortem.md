---
description: Postmortem analysis of cluster failures → OPTIMIZE PROMPTS to work for EVERY use case
argument-hint: <cluster-id or "recent" or "dump">
---

**PURPOSE: Find prompt weaknesses and FIX THEM so agents work out of the box for EVERY use case.**

This is NOT about debugging infrastructure. This is about making prompts BULLETPROOF.

## The Goal

Every cluster failure is a **prompt improvement opportunity**. Analyze what went wrong → Fix the prompt → Never see this failure pattern again.

## Input

`$ARGUMENTS` can be:

- **Cluster ID**: Analyze specific cluster
- **"recent"**: Find most recent clusters
- **"dump"**: User will paste logs directly

## Step 1: Get the Data

```bash
# List recent clusters
zeroshot list --json | jq '.[-5:]'

# Get cluster status and logs
zeroshot status $CLUSTER_ID --json
zeroshot logs $CLUSTER_ID 2>&1

# Query ledger for agent messages
sqlite3 ~/.zeroshot/clusters/$CLUSTER_ID/ledger.db "
  SELECT timestamp, topic, sender, content_text
  FROM messages
  ORDER BY timestamp ASC;
"
```

## Step 2: Identify What Agent Did Wrong

**READ THE LOGS. What SPECIFICALLY went wrong?**

| Failure Pattern         | Evidence                             | Prompt Gap                               |
| ----------------------- | ------------------------------------ | ---------------------------------------- |
| **Wrong files edited**  | Agent edited unrelated files         | Prompt doesn't scope file targets        |
| **Missed requirements** | Output missing key functionality     | Prompt doesn't emphasize requirements    |
| **Broke existing code** | Tests failed after changes           | Prompt doesn't enforce verification      |
| **Infinite loop**       | Worker/validator cycle >5 iterations | Validation criteria too strict or vague  |
| **Wrong approach**      | Used deprecated API, bad pattern     | Prompt missing technical constraints     |
| **Incomplete work**     | Partial implementation               | Prompt doesn't define "done" clearly     |
| **Hallucinated APIs**   | Called non-existent functions        | Prompt doesn't ground in actual codebase |
| **Ignored context**     | Didn't read provided files           | Context injection not working            |
| **Over-engineering**    | Added unnecessary complexity         | Prompt doesn't enforce simplicity        |
| **Under-testing**       | No tests written                     | Prompt doesn't require tests             |

## Step 3: Trace the Prompt Chain

**Which prompt caused this behavior?**

```
ISSUE_OPENED (user input)
    ↓
conductor-bootstrap.json → junior-conductor/senior-conductor prompts
    ↓
CLUSTER_OPERATIONS (classification)
    ↓
{config}.json → agent prompts (worker, validator, etc.)
    ↓
Agent behavior (good or bad)
```

**Find the weak link:**

1. **Conductor misclassified?** → Fix `cluster-templates/conductor-bootstrap.json`
2. **Wrong config loaded?** → Fix classification logic or config selection
3. **Worker did wrong thing?** → Fix worker prompt in config template
4. **Validator too strict/loose?** → Fix validation criteria
5. **Context missing?** → Fix context injection in agent config

## Step 4: Analyze Prompt Effectiveness

### For Each Agent That Failed:

**1. What was the prompt?**

```bash
# Find the config that was loaded
sqlite3 ledger.db "
  SELECT content_data FROM messages
  WHERE topic = 'CLUSTER_OPERATIONS' LIMIT 1;
" | jq -r '.operations[] | select(.action == "load_config") | .config'

# Read the actual prompt from that config
cat cluster-templates/base-templates/{config}.json | jq '.agents[] | select(.id == "worker") | .prompt'
```

**2. What did the agent actually do?**

- Read the agent's output from logs
- Compare intended behavior vs actual behavior

**3. Where did the prompt fail to guide?**

| Prompt Issue                 | Symptom                              | Fix                                 |
| ---------------------------- | ------------------------------------ | ----------------------------------- |
| **Too vague**                | Agent made random choices            | Add specific constraints            |
| **Too restrictive**          | Agent couldn't solve problem         | Loosen constraints, add flexibility |
| **Missing edge case**        | Agent broke on specific input        | Add explicit handling               |
| **Wrong emphasis**           | Agent focused on wrong thing         | Reorder priorities, use CAPS        |
| **No verification step**     | Agent declared done without checking | Add explicit verification           |
| **No examples**              | Agent misunderstood format           | Add concrete examples               |
| **Conflicting instructions** | Agent did inconsistent things        | Remove contradictions               |

## Step 5: Check Validation Criteria

**Validation failures are PROMPT BUGS, not agent bugs.**

If validator rejects good work:

- Validation criteria too strict
- Validation prompt doesn't understand the task

If validator approves bad work:

- Validation criteria too loose
- Validation prompt missing checks

```bash
# Find validation results
sqlite3 ledger.db "
  SELECT sender, content_text, content_data
  FROM messages
  WHERE topic = 'VALIDATION_RESULT';
"
```

**Questions to answer:**

1. Did validator check the RIGHT things?
2. Did validator understand the requirements?
3. Was rejection reason valid or false positive?
4. Was approval justified or false negative?

## Step 6: Generate Prompt Improvements

### Report Format

```markdown
# Prompt Analysis: [Cluster ID]

## Summary

- **Task**: [what user asked for]
- **Outcome**: [success/failure/partial]
- **Root Cause**: [which prompt failed and why]

---

## 🔴 FAILURE ANALYSIS

### What Went Wrong

> [Specific quote from logs showing the failure]

### Why It Went Wrong

[Analysis of which prompt instruction was missing/wrong/vague]

### The Prompt Gap
```

Current prompt says: "..."
But agent needed: "..."

```

---

## 📊 AGENT BEHAVIOR AUDIT

| Agent | Expected Behavior | Actual Behavior | Gap |
|-------|-------------------|-----------------|-----|
| conductor | Classify as ROUTINE/bugfix | Classified as TRIVIAL/feature | Wrong complexity |
| worker | Edit src/api.ts only | Edited 5 unrelated files | No file scoping |
| validator | Check API works | Only checked syntax | Missing functional test |

---

## 🔧 PROMPT FIXES

### Fix 1: [Config/Agent Name]

**File**: `cluster-templates/base-templates/{config}.json`

**Problem**: [What's wrong with current prompt]

**Current**:
```

[current prompt text]

```

**Fixed**:
```

[improved prompt text]

```

**Why This Fixes It**: [Explanation]

---

### Fix 2: [Config/Agent Name]
...

---

## 🎯 GENERALIZED IMPROVEMENTS

These fixes should apply to ALL configs, not just this one:

1. **[Pattern]**: [Improvement that prevents this class of failures]
2. ...

---

## ✅ VERIFICATION

After applying fixes, this cluster type should:
- [ ] [Specific behavior that should now work]
- [ ] [Another behavior]
```

## Step 7: Ask User What To Do

After presenting the analysis and proposed fixes, **ASK THE USER** what they want to do:

```
Use AskUserQuestion tool with options:
1. "Create GitHub issue" - Create issue with analysis and proposed fixes
2. "Apply fixes locally" - Apply fixes now, I'll test and commit
3. "Just the analysis" - Do nothing, I'll handle it manually
```

**If user chooses "Create GitHub issue":**

1. Create issue with title: `fix(prompts): [brief description of prompt gap]`
2. Body contains:
   - The full analysis report from Step 6
   - Proposed fixes with before/after diffs
   - Cluster ID that revealed the issue
   - Verification checklist
3. Label: `prompt-optimization`

**If user chooses "Apply fixes locally":**

1. Apply the prompt fixes to config files
2. Tell user to test with similar task and commit when ready

**If user chooses "Just the analysis":**

1. Do nothing - user has the report and will handle it

## Common Prompt Gaps

### File Scoping

```diff
- "Implement the feature"
+ "Implement the feature by editing ONLY files in src/. Do NOT modify tests/, docs/, or config files unless explicitly required."
```

### Verification Requirements

```diff
- "Complete the task"
+ "Complete the task. Before declaring done: 1) Run tests, 2) Verify the feature works manually, 3) Check for TypeScript errors"
```

### Output Format

```diff
- "Return the result"
+ "Return the result as JSON: { \"status\": \"success\"|\"failure\", \"files_changed\": [...], \"summary\": \"...\" }"
```

### Context Usage

```diff
- "Fix the bug"
+ "Fix the bug. The relevant code is in the CONTEXT section above. Read it carefully before making changes."
```

### Iteration Limits

```diff
- "Keep trying until it works"
+ "Make at most 3 attempts. If still failing after 3 attempts, report what's blocking you instead of continuing."
```

### Edge Case Handling

```diff
- "Handle errors appropriately"
+ "Handle errors: 1) Network errors → retry 3x with backoff, 2) Auth errors → fail immediately with clear message, 3) Validation errors → return details of what failed"
```

## Anti-Patterns to Fix

| Anti-Pattern         | Why It Fails             | Fix                              |
| -------------------- | ------------------------ | -------------------------------- |
| "Be careful"         | Too vague                | Specify WHAT to be careful about |
| "Use best practices" | Agent doesn't know which | List the specific practices      |
| "Don't break things" | Negative instruction     | "Verify X, Y, Z still work"      |
| "Be thorough"        | Unmeasurable             | "Check these 5 specific things"  |
| "Handle edge cases"  | Which ones?              | List the edge cases explicitly   |

## Checklist

- [ ] Identified SPECIFIC failure (not vague "it didn't work")
- [ ] Traced failure to SPECIFIC prompt/config
- [ ] Understood WHY prompt failed to guide agent
- [ ] Wrote CONCRETE fix (not "make it better")
- [ ] Fix is GENERALIZABLE (helps all similar tasks)
- [ ] Verified fix doesn't break other use cases
