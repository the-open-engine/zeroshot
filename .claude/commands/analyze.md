---
description: Deep analysis of zeroshot cluster issues - provider/isolation root causes FIRST, then agent prompts
argument-hint: <cluster-id or "recent" or "dump">
---

Analyze zeroshot cluster with **root cause analysis**. Check provider failures and isolation issues BEFORE blaming agent prompts.

**Three Root Cause Categories:**

- **A: PROVIDER BROKE** - API rate limit, timeout, spawn hang (like opencode 429)
- **B: ISOLATION MISLED** - Worktree/docker/PR mode gave wrong context
- **C: AGENT ISSUE** - Provider and isolation worked, agent made a mistake

## Input

`$ARGUMENTS` can be:

- **Cluster ID**: Analyze specific cluster (e.g., `zs_abc123`)
- **"recent"**: Find most recent clusters
- **"dump"**: User will paste logs directly
- **Empty**: Prompt for cluster ID or show recent clusters

## Step 1: Retrieve Cluster Data

### Option A: From Ledger (cluster-id or "recent")

```bash
# List recent clusters
zeroshot list --json | jq '.[-5:]'

# Get cluster status
zeroshot status $CLUSTER_ID --json

# Get cluster logs
zeroshot logs $CLUSTER_ID 2>&1 | tail -200

# Query ledger directly (if available)
sqlite3 ~/.zeroshot/clusters/$CLUSTER_ID/ledger.db "
  SELECT timestamp, topic, sender, content_text, content_data
  FROM messages
  ORDER BY timestamp ASC
  LIMIT 100;
"
```

### Option B: From Log Dump

If user pastes logs, extract:

1. **Cluster ID** from header
2. **Agent lifecycle events** (started, completed, failed)
3. **Message bus traffic** (topic publications)
4. **Provider errors** (rate limits, timeouts)
5. **Isolation mode** (worktree, docker, pr, ship)

## Step 2: Provider State Check (BEFORE BLAMING AGENT!)

**CRITICAL**: Check provider health FIRST. Agent failures are often API issues.

### Known Provider Failure Patterns

| Pattern               | Evidence                                    | Root Cause                       |
| --------------------- | ------------------------------------------- | -------------------------------- |
| **Rate limit (429)**  | `429 Too Many Requests`, `rate_limit_error` | API throttling                   |
| **Spawn hang**        | Agent started but no output for >60s        | Provider CLI hung (opencode bug) |
| **Timeout**           | `ETIMEDOUT`, `socket hang up`               | Network/API timeout              |
| **Auth failure**      | `401 Unauthorized`, `invalid_api_key`       | Credential issue                 |
| **Model unavailable** | `model_not_available`, `overloaded`         | API capacity                     |
| **Context overflow**  | `context_length_exceeded`                   | Prompt too large                 |

### Check Provider Logs

```bash
# Check for rate limits
grep -i "429\|rate.limit\|too.many.requests" cluster.log

# Check for timeouts
grep -i "timeout\|ETIMEDOUT\|hang\|spawn" cluster.log

# Check for auth issues
grep -i "401\|unauthorized\|api.key\|credential" cluster.log

# Check spawn timing (>60s = hang)
grep "spawning\|started" cluster.log | head -10
```

### If ANY provider issue found → Agent is NOT at fault!

**DO NOT CONTINUE TO PROMPT ANALYSIS** if provider was broken.

---

## Step 3: Isolation State Check

### Isolation Mode Issues

| Mode           | Issue                     | Evidence                             |
| -------------- | ------------------------- | ------------------------------------ |
| **--worktree** | Worktree creation failed  | `fatal: could not create worktree`   |
| **--worktree** | Wrong branch checked out  | `HEAD detached`, unexpected branch   |
| **--docker**   | Container failed to start | `docker: Error response from daemon` |
| **--docker**   | Volume mount failed       | `cannot mount`, permission denied    |
| **--pr**       | PR creation failed        | `gh pr create` error                 |
| **--pr**       | Branch push failed        | `rejected`, `non-fast-forward`       |
| **--ship**     | Merge failed              | `merge conflict`, CI failed          |

### Check Isolation Logs

```bash
# Check worktree issues
grep -i "worktree\|branch\|checkout" cluster.log

# Check docker issues
grep -i "docker\|container\|volume\|mount" cluster.log

# Check PR/git issues
grep -i "pr\|push\|merge\|conflict" cluster.log
```

---

## Step 4: Message Flow Analysis

### Expected Flow (Happy Path)

```
ISSUE_OPENED (task input)
    ↓
CLUSTER_OPERATIONS (conductor classification)
    ↓
[config loaded, agents spawned]
    ↓
IMPLEMENTATION_READY (worker completed)
    ↓
VALIDATION_RESULT (validator approved/rejected)
    ↓
[if rejected: IMPLEMENTATION_READY again]
    ↓
CLUSTER_OPERATIONS_SUCCESS (cluster done)
```

### Find Flow Breaks

```bash
# Trace message topics
sqlite3 ledger.db "
  SELECT timestamp, topic, sender
  FROM messages
  WHERE topic IN (
    'ISSUE_OPENED',
    'CLUSTER_OPERATIONS',
    'IMPLEMENTATION_READY',
    'VALIDATION_RESULT',
    'CLUSTER_OPERATIONS_SUCCESS',
    'CLUSTER_FAILED',
    'AGENT_ERROR'
  )
  ORDER BY timestamp;
"

# Find errors
sqlite3 ledger.db "
  SELECT timestamp, sender, content_text, content_data
  FROM messages
  WHERE topic IN ('AGENT_ERROR', 'CLUSTER_FAILED', 'CLUSTER_OPERATIONS_FAILED')
  ORDER BY timestamp;
"

# Find validation rejections
sqlite3 ledger.db "
  SELECT timestamp, sender, content_data
  FROM messages
  WHERE topic = 'VALIDATION_RESULT'
  AND content_data LIKE '%\"approved\":false%'
  ORDER BY timestamp;
"
```

---

## Step 5: Conductor Classification Analysis

### Check Classification Quality

```bash
# Get conductor's classification
sqlite3 ledger.db "
  SELECT content_text, content_data
  FROM messages
  WHERE topic = 'CLUSTER_OPERATIONS'
  ORDER BY timestamp ASC
  LIMIT 1;
"
```

**Classification Issues:**

| Issue                    | Evidence                               | Impact                      |
| ------------------------ | -------------------------------------- | --------------------------- |
| **Under-classified**     | TRIVIAL but needed multiple validators | Slow, missing quality gates |
| **Over-classified**      | CRITICAL but was simple change         | Wasted resources            |
| **Wrong taskType**       | Feature classified as refactor         | Wrong agent roles spawned   |
| **UNCERTAIN escalation** | Conductor couldn't decide              | Senior re-evaluation needed |

---

## Step 6: Token Usage Analysis

```bash
# Get token usage per agent
sqlite3 ledger.db "
  SELECT sender, SUM(json_extract(content_data, '$.input_tokens')) as input,
         SUM(json_extract(content_data, '$.output_tokens')) as output
  FROM messages
  WHERE topic = 'TOKEN_USAGE'
  GROUP BY sender;
"

# Get total cost
zeroshot status $CLUSTER_ID --json | jq '.tokenUsage'
```

---

## Step 7: Generate Analysis Report

```markdown
# Zeroshot Cluster Analysis: [Cluster ID]

## Summary

- **Cluster**: [id]
- **Task**: [original issue/task]
- **Isolation Mode**: [none/worktree/docker/pr/ship]
- **Status**: [completed/failed/running]
- **Duration**: [start → end]
- **Total Tokens**: [input + output]

---

## 🔴 ROOT CAUSE DETERMINATION

### Category A: PROVIDER BROKE

**API or CLI failed - NOT agent's fault.**

Evidence:

> [Quote showing provider failure]

**FIX REQUIRED**: [Provider-level fix]

---

### Category B: ISOLATION MISLED

**Isolation mode gave wrong context.**

Evidence:

> [Quote showing isolation issue]

**FIX REQUIRED**: [Isolation config fix]

---

### Category C: AGENT ISSUE

**Provider and isolation worked. Agent made mistake.**

Evidence:

> [Quote showing agent error]

**FIX REQUIRED**: [Prompt/config change]

---

**CHOSEN CATEGORY**: [ A | B | C ]

---

## 📊 Message Flow Analysis

| Timestamp | Topic                | Sender    | Status      |
| --------- | -------------------- | --------- | ----------- |
| ...       | ISSUE_OPENED         | user      | ✅          |
| ...       | CLUSTER_OPERATIONS   | conductor | ✅          |
| ...       | IMPLEMENTATION_READY | worker    | ✅          |
| ...       | VALIDATION_RESULT    | validator | ❌ rejected |
| ...       | ...                  | ...       | ...         |

### Flow Breaks Found

1. [Where flow broke and why]

---

## 🔍 Conductor Classification

- **Complexity**: [TRIVIAL/ROUTINE/COMPLEX/CRITICAL]
- **Task Type**: [feature/bugfix/refactor/docs]
- **Config Loaded**: [config name]

### Classification Quality

- [ ] Appropriate complexity level
- [ ] Correct task type
- [ ] Right number of validators

---

## 💰 Token Usage

| Agent       | Role           | Input | Output | Cost   |
| ----------- | -------------- | ----- | ------ | ------ |
| conductor   | classification | X     | Y      | $Z     |
| worker-1    | implementation | X     | Y      | $Z     |
| validator-1 | validation     | X     | Y      | $Z     |
| **Total**   |                | **X** | **Y**  | **$Z** |

---

## 🔧 Recommended Fixes

### Provider Fixes

1. [If category A]

### Isolation Fixes

1. [If category B]

### Agent/Prompt Fixes

1. [If category C]

### Template Fixes

1. [Cluster template improvements]

---

## 🎯 Priority Actions

1. **CRITICAL**: [Most important fix]
2. **HIGH**: [Second priority]
3. **MEDIUM**: [Can wait]
```

## Analysis Checklist

### Step 1: Data Retrieval

- [ ] Retrieved cluster status
- [ ] Retrieved cluster logs
- [ ] Queried ledger messages

### Step 2: Provider Check (DO THIS FIRST!)

- [ ] Checked for rate limits (429)
- [ ] Checked for spawn hangs (>60s no output)
- [ ] Checked for timeouts
- [ ] Checked for auth failures

### Step 3: Isolation Check

- [ ] Verified worktree created successfully (if --worktree)
- [ ] Verified container started (if --docker)
- [ ] Verified PR created (if --pr)
- [ ] Verified merge succeeded (if --ship)

### Step 4: Message Flow

- [ ] Traced ISSUE_OPENED → CLUSTER_OPERATIONS → IMPLEMENTATION_READY → VALIDATION_RESULT
- [ ] Found flow breaks
- [ ] Identified failed validations

### Step 5: Classification

- [ ] Verified conductor classification was appropriate
- [ ] Checked if taskType matched actual work

### Step 6: Token Usage

- [ ] Calculated per-agent token usage
- [ ] Identified cost anomalies

---

## ROOT CAUSE DECISION TREE

```
START
  │
  ▼
[1] Did provider work? (No 429, no hang, no timeout?)
  │
  ├─ NO → **CATEGORY A: PROVIDER BROKE**
  │        Fix: Provider config, retry logic, rate limit handling
  │
  └─ YES
       │
       ▼
[2] Did isolation work? (Worktree/docker/PR succeeded?)
  │
  ├─ NO → **CATEGORY B: ISOLATION MISLED**
  │        Fix: Isolation mode, git config, docker setup
  │
  └─ YES
       │
       ▼
[3] Did agents succeed?
  │
  ├─ YES → No problem (or user expectation issue)
  │
  └─ NO → **CATEGORY C: AGENT ISSUE**
          Fix: Prompts, triggers, validation criteria
```

---

## Common Failure Patterns

### Provider Failures

| Pattern               | Fix                                      |
| --------------------- | ---------------------------------------- |
| Rate limit (429)      | Add retry with backoff, check API quotas |
| Spawn hang (opencode) | Already fixed in spawn timeout PR        |
| Context overflow      | Reduce prompt size, use summarization    |
| Model unavailable     | Fall back to different model             |

### Isolation Failures

| Pattern                         | Fix                      |
| ------------------------------- | ------------------------ |
| Worktree on uncommitted changes | WIP commit first         |
| Docker volume permissions       | Check UID mapping        |
| PR branch exists                | Use unique branch names  |
| Merge conflicts                 | Better rebasing strategy |

### Agent Failures

| Pattern                   | Fix                              |
| ------------------------- | -------------------------------- |
| Validation always rejects | Loosen validation criteria       |
| Worker loops forever      | Add max iteration limit          |
| Wrong files edited        | Improve file targeting in prompt |
| Tests not run             | Add explicit verification step   |

---

## Related Commands

- `/postmortem` - General debugging postmortem
- `/status` - Quick cluster status check
