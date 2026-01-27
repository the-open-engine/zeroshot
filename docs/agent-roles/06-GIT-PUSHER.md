# Git Pusher Role (Completion Detector)

> Automated PR/MR creation and merging

## Overview

The **git-pusher** role is a specialized **completion-detector** that handles git operations after all validators approve. It commits changes, pushes to remote, creates PRs/MRs, and optionally auto-merges.

## When Used

- Injected when: `--pr` or `--ship` flags are used
- Replaces: Default completion-detector
- Platforms: GitHub, GitLab, Azure DevOps, Gitea

## Agent Configuration

```json
{
  "id": "git-pusher",
  "role": "completion-detector",
  "modelLevel": "level2"
}
```

## Trigger Logic

Waits for ALL validators to approve with REAL evidence:

```javascript
const validators = cluster.getAgentsByRole('validator');
const lastPush = ledger.findLast({ topic: 'IMPLEMENTATION_READY' });

// Need implementation to be ready
if (!lastPush) return false;

// If no validators, proceed
if (validators.length === 0) return true;

// Wait for all validators to respond
const results = ledger.query({ topic: 'VALIDATION_RESULT', since: lastPush.timestamp });
if (results.length < validators.length) return false;

// All must approve
const allApproved = results.every(
  (r) => r.content?.data?.approved === 'true' || r.content?.data?.approved === true
);
if (!allApproved) return false;

// Evidence must be real (not fake approvals)
const hasRealEvidence = results.every((r) => {
  const criteria = r.content?.data?.criteriaResults || [];
  return criteria.every((c) => {
    return (
      c.evidence?.command &&
      typeof c.evidence?.exitCode === 'number' &&
      c.evidence?.output?.length > 10
    );
  });
});

return hasRealEvidence;
```

## Context Strategy

```json
{
  "sources": [
    { "topic": "ISSUE_OPENED", "limit": 1 },
    { "topic": "IMPLEMENTATION_READY", "since": "last_agent_start", "limit": 1 },
    { "topic": "VALIDATION_RESULT", "since": "last_agent_start", "limit": 10 }
  ],
  "format": "chronological",
  "maxTokens": 100000
}
```

---

## Platform-Specific Configurations

### GitHub

````markdown
## MANDATORY STEPS

### STEP 1: Stage ALL changes (MANDATORY)

```bash
git add -A
```
````

### STEP 2: Check what's staged

```bash
git status
```

If nothing to commit, output JSON with pr_url: null and stop.

### STEP 3: Commit the changes (MANDATORY)

```bash
git commit -m "feat: implement #{{issue_number}} - {{issue_title}}"
```

### STEP 4: Push to origin (MANDATORY)

```bash
git push -u origin HEAD
```

⚠️ AFTER PUSH YOU ARE NOT DONE! CONTINUE TO STEP 5! ⚠️

### STEP 5: CREATE THE PR (MANDATORY)

```bash
gh pr create --title "feat: {{issue_title}}" --body "Closes #{{issue_number}}"
```

🚨 YOU MUST RUN `gh pr create`! Outputting a link is NOT creating a PR! 🚨

⚠️ AFTER PR CREATION YOU ARE NOT DONE! CONTINUE TO STEP 6! ⚠️

### STEP 6: MERGE THE PR (MANDATORY)

```bash
gh pr merge --merge --auto
```

If it fails, try:

```bash
gh pr merge --merge
```

🚨 IF MERGE FAILS DUE TO CONFLICTS:
a) Pull latest main and rebase
b) Resolve conflicts
c) Force push the resolved branch
d) Retry merge

## Final Output

```json
{ "pr_url": "https://github.com/owner/repo/pull/123", "pr_number": 123, "merged": true }
```

````

### GitLab

```markdown
### STEP 5: CREATE THE MR (MANDATORY)
```bash
glab mr create --title "feat: {{issue_title}}" --description "Closes #{{issue_number}}"
````

### STEP 6: MERGE THE MR (MANDATORY)

```bash
glab mr merge --auto-merge
```

If it fails:

```bash
glab mr merge
```

## Final Output

```json
{ "mr_url": "https://gitlab.com/owner/repo/-/merge_requests/123", "mr_number": 123, "merged": true }
```

````

### Azure DevOps

```markdown
### STEP 5: CREATE THE PR (MANDATORY)
```bash
az repos pr create --title "feat: {{issue_title}}" --description "Closes #{{issue_number}}"
````

💡 IMPORTANT: Extract the PR ID from the output for step 6.

### STEP 6: SET AUTO-COMPLETE (MANDATORY)

```bash
az repos pr update --id <PR_ID> --auto-complete true
```

Replace <PR_ID> with actual PR number from step 5.

If auto-complete not available:

```bash
az repos pr update --id <PR_ID> --status completed
```

## Final Output

```json
{
  "pr_url": "https://dev.azure.com/org/project/_git/repo/pullrequest/123",
  "pr_number": 123,
  "merged": false,
  "auto_complete": true
}
```

````

### Gitea

```markdown
### STEP 5: CREATE THE PR (MANDATORY)
```bash
tea pulls create --title "feat: {{issue_title}}" --description "Closes #{{issue_number}}"
````

### STEP 6: MERGE THE PR (MANDATORY)

```bash
tea pulls merge --style merge
```

## Final Output

```json
{ "pr_url": "https://gitea.example.com/owner/repo/pulls/123", "pr_number": 123, "merged": true }
```

````

---

## Full Prompt Template (GitHub Example)

```markdown
🚨 CRITICAL: ALL VALIDATORS APPROVED. YOU MUST CREATE A PR AND GET IT MERGED.
DO NOT STOP UNTIL THE PR IS MERGED. 🚨

## MANDATORY STEPS - EXECUTE EACH ONE IN ORDER - DO NOT SKIP ANY STEP

### STEP 1: Stage ALL changes (MANDATORY)
```bash
git add -A
````

Run this command. Do not skip it.

### STEP 2: Check what's staged

```bash
git status
```

Run this. If nothing to commit, output JSON with pr_url: null and stop.

### STEP 3: Commit the changes (MANDATORY if there are changes)

```bash
git commit -m "feat: implement #{{issue_number}} - {{issue_title}}"
```

Run this command. Do not skip it.

### STEP 4: Push to origin (MANDATORY)

```bash
git push -u origin HEAD
```

Run this. If it fails, check the error and fix it.

⚠️ AFTER PUSH YOU ARE NOT DONE! CONTINUE TO STEP 5! ⚠️

### STEP 5: CREATE THE PR (MANDATORY - YOU MUST RUN THIS COMMAND)

```bash
gh pr create --title "feat: {{issue_title}}" --body "Closes #{{issue_number}}"
```

🚨 YOU MUST RUN `gh pr create`! Outputting a link is NOT creating a PR! 🚨
The push output shows a "Create a pull request" link - IGNORE IT.
You MUST run the `gh pr create` command above. Save the actual PR URL from the output.

⚠️ AFTER PR CREATION YOU ARE NOT DONE! CONTINUE TO STEP 6! ⚠️

### STEP 6: MERGE THE PR (MANDATORY - THIS IS NOT OPTIONAL)

```bash
gh pr merge --merge --auto
```

This sets auto-merge. If it fails (e.g., no auto-merge enabled), try:

```bash
gh pr merge --merge
```

🚨 IF MERGE FAILS DUE TO CONFLICTS - YOU MUST RESOLVE THEM:
a) Pull latest main and rebase:

```bash
git fetch origin main
git rebase origin/main
```

b) If conflicts appear - RESOLVE THEM IMMEDIATELY:

- Read the conflicting files
- Make intelligent decisions about what code to keep
- Edit the files to resolve conflicts
- `git add <resolved-files>`
- `git rebase --continue`
  c) Force push the resolved branch:

```bash
git push --force-with-lease
```

d) Retry merge:

```bash
gh pr merge --merge
```

REPEAT UNTIL MERGED. DO NOT GIVE UP. DO NOT SKIP. THE PR MUST BE MERGED.
If merge is blocked by CI, wait and retry. If blocked by reviews, set auto-merge.

## CRITICAL RULES

- Execute EVERY step in order (1, 2, 3, 4, 5, 6)
- Do NOT skip git add -A
- Do NOT skip git commit
- Do NOT skip gh pr create - THE TASK IS NOT DONE UNTIL PR EXISTS
- Do NOT skip gh pr merge - THE TASK IS NOT DONE UNTIL PR IS MERGED
- If push fails, debug and fix it
- If PR creation fails, debug and fix it
- If merge fails, debug and fix it
- DO NOT OUTPUT JSON UNTIL PR IS MERGED
- A link from git push is NOT a PR - you must run gh pr create

## Final Output

ONLY after the PR is MERGED, output:

```json
{ "pr_url": "https://github.com/owner/repo/pull/123", "pr_number": 123, "merged": true }
```

If truly no changes exist, output:

```json
{ "pr_url": null, "pr_number": null, "merged": false }
```

```

---

## Workflow Position

```

IMPLEMENTATION*READY
│
▼
┌────────────────┐
│ VALIDATORS │ (parallel)
└────────┬───────┘
│
▼ All approved with evidence?
│
├── NO ──► Worker retries
│
▼ YES
┌────────────────┐
│ GIT-PUSHER │ ← You are here
└────────┬───────┘
│
▼ Steps 1-6 executed
│
┌────┴────┐
│ │
Success Conflict
│ │
▼ ▼
PR_CREATED Resolve,
│ retry
▼ │
CLUSTER* └───────►
COMPLETE

```

## Key Behaviors

1. **Evidence Validation** - Only triggers when validators provide real evidence (command + exitCode + output)
2. **Step-by-Step Execution** - Must execute ALL 6 steps in order
3. **Conflict Resolution** - Must resolve merge conflicts, not give up
4. **Retry Logic** - If merge blocked, wait and retry
5. **Platform Detection** - Auto-selects CLI commands based on git remote

## --ship vs --pr

| Flag | Behavior |
|------|----------|
| `--pr` | Create PR, set auto-merge, complete |
| `--ship` | Create PR, merge immediately, wait for CI if needed |

With `--ship`, the cluster doesn't complete until the PR is actually merged (not just created).

## Supported Platforms

| Platform | CLI Tool | PR Create | Merge |
|----------|----------|-----------|-------|
| GitHub | `gh` | `gh pr create` | `gh pr merge --merge` |
| GitLab | `glab` | `glab mr create` | `glab mr merge` |
| Azure DevOps | `az repos` | `az repos pr create` | `az repos pr update --auto-complete` |
| Gitea | `tea` | `tea pulls create` | `tea pulls merge` |
```
