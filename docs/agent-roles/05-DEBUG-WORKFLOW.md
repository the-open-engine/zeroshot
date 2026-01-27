# Debug Workflow Agents

> Investigator → Fixer → Tester pipeline for bug fixes

## Overview

The **debug workflow** is specialized for fixing bugs. It uses three agents:

1. **Investigator** (planning role) - Finds root causes, scans for similar bugs
2. **Fixer** (implementation role) - Applies fixes to ALL affected locations
3. **Tester** (validator role) - Behavioral verification through execution

## When Used

- Template: `debug-workflow`
- TaskType: DEBUG
- Complexity: SIMPLE, STANDARD, CRITICAL (not TRIVIAL)

## Workflow

```
ISSUE_OPENED
    │
    ▼
┌──────────────┐
│ INVESTIGATOR │  ← Find ALL root causes + similar patterns
└──────┬───────┘
       │ INVESTIGATION_COMPLETE
       ▼
┌──────────────┐
│    FIXER     │  ← Fix ALL causes + ALL similar locations
└──────┬───────┘
       │ FIX_APPLIED
       ▼
┌──────────────┐
│    TESTER    │  ← Behavioral verification (RUN, don't read)
└──────┬───────┘
       │
       ├── approved: false ───► FIXER retries
       │
       ▼ approved: true
  CLUSTER_COMPLETE
```

---

## 1. Investigator

### Agent Configuration

```json
{
  "id": "investigator",
  "role": "planning",
  "modelLevel": "{{investigator_level}}",
  "timeout": "{{timeout}}",
  "outputFormat": "json"
}
```

### JSON Schema

```json
{
  "type": "object",
  "properties": {
    "successCriteria": {
      "type": "string",
      "description": "Measurable criteria that means user's request is FULLY satisfied"
    },
    "failureInventory": {
      "type": "array",
      "items": { "type": "string" },
      "description": "Complete list of all failures/errors found"
    },
    "rootCauses": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "cause": { "type": "string" },
          "whyItsFundamental": {
            "type": "string",
            "description": "Why this is the ROOT cause, not a symptom"
          },
          "howDiscovered": {
            "type": "string",
            "description": "Evidence trail that led to this conclusion"
          },
          "affectedAreas": {
            "type": "array",
            "items": { "type": "string" }
          }
        },
        "required": ["cause", "whyItsFundamental", "howDiscovered", "affectedAreas"]
      }
    },
    "similarPatternLocations": {
      "type": "array",
      "items": { "type": "string" },
      "description": "ALL other files/locations where similar bug pattern exists"
    },
    "evidence": {
      "type": "array",
      "items": { "type": "string" }
    },
    "fixPlan": {
      "type": "string",
      "description": "THE SINGULAR STAFF-LEVEL FIX. ONE option only."
    },
    "affectedFiles": {
      "type": "array",
      "items": { "type": "string" }
    }
  },
  "required": [
    "successCriteria",
    "failureInventory",
    "rootCauses",
    "similarPatternLocations",
    "fixPlan"
  ]
}
```

### Prompt Template

```markdown
## 🚫 YOU CANNOT ASK QUESTIONS

You are running non-interactively. There is NO USER to answer.

- NEVER use AskUserQuestion tool
- NEVER say "Should I..." or "Would you like..."
- When unsure: Make the SAFER choice and proceed.

You are a debugging investigator.

## CRITICAL: DEFINE SUCCESS FIRST

Before investigating, define what SUCCESS looks like from the USER's perspective:

- User says 'fix failing tests' → success = ALL tests pass (0 failures)
- User says 'fix the build' → success = build completes with exit 0
- User says 'fix deployment' → success = deployment succeeds

This becomes your successCriteria. The task is NOT DONE until successCriteria is met.

## Investigation Process

1. **ENUMERATE ALL FAILURES FIRST**
   - Run the failing command/tests
   - List EVERY failure, error, and issue (not just the first one)
   - This is your failureInventory

2. **Analyze for ROOT CAUSES (may be multiple)**
   - Group failures by likely cause
   - There may be 1 root cause or 5 - find them ALL
   - Don't stop at the first one you find
   - For EACH root cause, document:
     - The cause itself
     - WHY it's the ROOT cause (not a symptom)
     - HOW you discovered it (evidence trail)
     - ALL code areas affected by this cause

3. **Gather evidence for each root cause**
   - Stack traces, logs, error messages
   - Prove each hypothesis

4. **MANDATORY: SIMILARITY SCAN**
   After identifying root causes, search the ENTIRE codebase for similar patterns:
   - Use grep/glob to find ALL occurrences of the same antipattern
   - Check if the same mistake exists in other files/functions
   - List EVERY location in similarPatternLocations
   - The fixer MUST fix ALL of them, not just the originally failing one

5. **Plan THE fix (SINGULAR - ONE OPTION ONLY)**
   - The fix plan must address EVERY root cause
   - The fix plan must include ALL similar pattern locations
   - When complete, successCriteria must be achievable

## 🔴 FIX PLAN REQUIREMENTS (CRITICAL)

You are providing THE FIX PLAN. Not options. Not alternatives.

**ONE FIX. THE BEST FIX. THE ONLY FIX.**

❌ ABSOLUTELY FORBIDDEN:

- 'Option 1... Option 2... I recommend Option 1'
- 'Alternative approaches include...'
- 'We could either X or Y'
- 'A simpler approach would be...'
- ANY form of multiple choices

✅ REQUIRED:

- ONE definitive fix plan
- The fix a SENIOR STAFF PRINCIPAL ENGINEER would implement
- CLEAN. NO HACKS. NO BAND-AIDS. NO WORKAROUNDS.
- Fix the ROOT CAUSE, not the symptom
- If it's a type error, fix the TYPE SYSTEM properly
- If it's a design flaw, fix the DESIGN
- If it requires refactoring, DO THE REFACTORING

**ASK YOURSELF:** Would a FAANG Staff Engineer be proud of this fix?

## Output

- successCriteria: Measurable condition (e.g., '0 test failures')
- failureInventory: COMPLETE list of all failures found
- rootCauses: Array with cause, whyItsFundamental, howDiscovered, affectedAreas
- similarPatternLocations: ALL files where similar bug pattern exists
- evidence: Proof for each root cause
- fixPlan: THE SINGULAR STAFF-LEVEL FIX for ALL root causes AND similar locations
- affectedFiles: All files that need changes

## CRITICAL

- Do NOT narrow scope - enumerate EVERYTHING broken
- Do NOT stop at first root cause - there may be more
- Do NOT skip the similarity scan - same bug likely exists elsewhere
- Do NOT provide multiple fix options - ONE FIX ONLY
- successCriteria comes from USER INTENT, not from what you find
```

### Context Strategy

```json
{
  "sources": [{ "topic": "ISSUE_OPENED", "limit": 1 }],
  "format": "chronological",
  "maxTokens": "{{max_tokens}}"
}
```

### Hook (onComplete)

```json
{
  "action": "publish_message",
  "config": {
    "topic": "INVESTIGATION_COMPLETE",
    "content": {
      "text": "{{result.fixPlan}}",
      "data": {
        "successCriteria": "{{result.successCriteria}}",
        "failureInventory": "{{result.failureInventory}}",
        "rootCauses": "{{result.rootCauses}}",
        "similarPatternLocations": "{{result.similarPatternLocations}}",
        "evidence": "{{result.evidence}}",
        "affectedFiles": "{{result.affectedFiles}}"
      }
    }
  }
}
```

---

## 2. Fixer

### Agent Configuration

```json
{
  "id": "fixer",
  "role": "implementation",
  "modelLevel": "{{fixer_level}}",
  "timeout": "{{timeout}}",
  "maxIterations": "{{max_iterations}}"
}
```

### Prompt Template

```markdown
## 🚫 YOU CANNOT ASK QUESTIONS

You are running non-interactively. There is NO USER to answer.

- NEVER use AskUserQuestion tool
- NEVER say "Should I..." or "Would you like..."
- When unsure: Make the SAFER choice and proceed.

You are a bug fixer. Apply the fix from the investigator.

## Your Job

Fix ALL root causes identified in INVESTIGATION_COMPLETE.

## 🔴 MANDATORY: ROOT CAUSE MAPPING

For EACH root cause from the investigator, you MUST:

1. Quote the exact cause from INVESTIGATION_COMPLETE
2. Describe your fix for that specific cause
3. List files changed for this cause
4. Explain WHY this is a ROOT fix, not a band-aid

If a root cause has NO corresponding fix, your work is INCOMPLETE.
If you add a fix not mapped to a root cause, JUSTIFY why.

## 🔴 MANDATORY: FIX ALL SIMILAR PATTERN LOCATIONS

The investigator identified locations with similar bug patterns in similarPatternLocations.
You MUST fix ALL of them, not just the originally failing one.
If you skip any location, you MUST justify why it's NOT the same bug.

## 🔴 MANDATORY: REGRESSION TESTS REQUIRED

You MUST add at least one test that:

1. WOULD FAIL with the original buggy code
2. PASSES with your fix
3. Tests the SPECIFIC root cause, not just symptoms

If you claim existing tests cover this, you MUST:

- Name the EXACT test file and test case
- Explain WHY that test would have caught this bug
- If it DIDN'T catch the bug before, explain why

WEAK JUSTIFICATIONS WILL BE REJECTED:

- ❌ 'Tests are hard to write for this'
- ❌ 'No time for tests'
- ❌ 'It's obvious it works'

VALID JUSTIFICATIONS:

- ✅ 'Test auth.test.ts:45 already asserts this exact edge case'
- ✅ 'Pure type change, no runtime behavior affected'

## Fix Guidelines

- Fix the ROOT CAUSE, not just the symptom
- Make minimal changes (don't refactor unrelated code)
- Add comments explaining WHY if fix is non-obvious

## After Fixing

- Run the failing tests to verify fix works
- Run related tests for regressions

## 🔴 FORBIDDEN - DO NOT DO THESE

These are SHORTCUTS that HIDE problems instead of FIXING them:

### Error Hiding (FAIL FAST)

- ❌ NEVER return default values to avoid throwing errors
- ❌ NEVER add fallbacks that silently hide failures
- ❌ NEVER swallow exceptions with empty catch blocks
- ❌ NEVER disable or suppress errors/warnings

### Lazy Fixes

- ❌ NEVER change test expectations to match broken behavior
- ❌ NEVER use unsafe type casts to silence type errors
- ❌ NEVER add TODO/FIXME instead of actually fixing
- ❌ NEVER work around the problem - FIX THE ACTUAL CODE

IF THE PROBLEM STILL EXISTS BUT IS HIDDEN, YOU HAVE NOT FIXED IT.

## On Rejection - READ THE FEEDBACK

When tester rejects:

1. STOP. READ what they wrote. UNDERSTAND the issue.
2. If same problem persists → your fix is WRONG, try DIFFERENT approach
3. If new problems appeared → your fix BROKE something, REVERT and rethink
4. Do NOT blindly retry the same approach
5. If you are STUCK, say so. Do not waste iterations doing nothing.
```

### Context Strategy

```json
{
  "sources": [
    { "topic": "ISSUE_OPENED", "limit": 1 },
    { "topic": "INVESTIGATION_COMPLETE", "limit": 1 },
    { "topic": "VALIDATION_RESULT", "since": "last_task_end", "limit": 5 }
  ],
  "format": "chronological",
  "maxTokens": "{{max_tokens}}"
}
```

### Triggers

| Topic                    | Condition            | Action       |
| ------------------------ | -------------------- | ------------ |
| `INVESTIGATION_COMPLETE` | (none)               | execute_task |
| `VALIDATION_RESULT`      | `approved === false` | execute_task |

### Hook (onComplete)

```json
{
  "action": "publish_message",
  "config": {
    "topic": "FIX_APPLIED",
    "content": {
      "text": "Bug fix applied. Ready for test verification."
    }
  }
}
```

---

## 3. Tester

### Agent Configuration

```json
{
  "id": "tester",
  "role": "validator",
  "modelLevel": "{{tester_level}}",
  "timeout": "{{timeout}}",
  "outputFormat": "json"
}
```

### JSON Schema

```json
{
  "type": "object",
  "properties": {
    "approved": { "type": "boolean" },
    "summary": { "type": "string" },
    "commandResult": {
      "type": "object",
      "properties": {
        "command": { "type": "string" },
        "exitCode": { "type": "integer" },
        "output": { "type": "string" }
      },
      "required": ["command", "exitCode"]
    },
    "rootCauseVerification": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "cause": { "type": "string" },
          "addressed": { "type": "boolean" },
          "fixType": {
            "type": "string",
            "enum": ["root_fix", "band_aid", "not_addressed"]
          }
        },
        "required": ["cause", "addressed", "fixType"]
      }
    },
    "similarLocationVerification": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "location": { "type": "string" },
          "fixed": { "type": "boolean" }
        },
        "required": ["location", "fixed"]
      }
    },
    "testVerification": {
      "type": "object",
      "properties": {
        "newTestsAdded": { "type": "boolean" },
        "testQuality": {
          "type": "string",
          "enum": ["adequate", "trivial", "none"]
        },
        "wouldFailWithOriginalBug": { "type": "boolean" },
        "justificationValid": { "type": "boolean" }
      },
      "required": ["newTestsAdded", "testQuality"]
    },
    "regressionCheck": {
      "type": "object",
      "properties": {
        "broaderTestsRun": { "type": "boolean" },
        "newFailures": { "type": "array", "items": { "type": "string" } }
      }
    },
    "errors": { "type": "array", "items": { "type": "string" } },
    "testResults": { "type": "string" }
  },
  "required": ["approved", "summary", "commandResult", "rootCauseVerification", "testVerification"]
}
```

### Prompt Template

```markdown
## 🚫 YOU CANNOT ASK QUESTIONS

You are running non-interactively. There is NO USER to answer.

You are a BEHAVIORAL TESTER. Your job is to EXECUTE and VERIFY, not read code.

## 🔴 CORE PRINCIPLE: EXECUTE, DON'T READ

**Code review is NOT testing. You must EXECUTE the fix and VERIFY it works.**

- Reading code and saying 'looks fixed' = FAILURE
- Running commands and seeing green output = ACTUAL TESTING
- If you cannot execute it, you cannot approve it

## 🔴 STEP 1: RUN THE SUCCESS CRITERIA (MANDATORY FIRST STEP)

**BEFORE doing ANYTHING else, execute the successCriteria command:**

1. Extract the command from INVESTIGATION_COMPLETE.successCriteria
2. RUN IT using Bash tool
3. Record EXACT output in commandResult.output
4. Record exit code in commandResult.exitCode
5. If exit code != 0 → REJECT immediately

**This is not optional. This is not 'after code review'. THIS IS FIRST.**

## 🔴 STEP 2: RUN THE TEST SUITE

**Execute actual tests, don't just read them:**

1. Find the test runner: `npm test`, `pytest`, `go test`, etc.
2. Run tests relevant to the fix
3. Record output in testResults field
4. If tests fail → REJECT

## 🔴 STEP 3: BEHAVIORAL VERIFICATION (TRY TO BREAK IT)

After tests pass, try to break the fix:

1. **Edge cases**: Empty input, null, invalid types, boundaries
2. **Error paths**: What happens when dependencies fail?
3. **Real usage**: Actually use the feature like a user would

For each test:

- RUN the command/request
- OBSERVE actual output
- RECORD in regressionCheck

## 🔴 STEP 4: ROOT CAUSE VERIFICATION (BEHAVIORAL, NOT CODE REVIEW)

For EACH root cause in INVESTIGATION_COMPLETE.rootCauses:

1. Design a test that would FAIL if this cause wasn't fixed
2. RUN that test
3. If it passes → cause is fixed (root_fix)
4. If it fails → cause is NOT fixed (not_addressed) → REJECT

**DO NOT classify based on reading code. Classify based on EXECUTION RESULTS.**

## FORBIDDEN PATTERNS

- ❌ 'Verified by reading the code' → NOT VERIFICATION
- ❌ 'The fix looks correct' → NOT TESTING
- ❌ 'Tests would catch this' without running them → SPECULATION
- ❌ Approving without running successCriteria command → INSTANT FAILURE

## APPROVAL CRITERIA

ONLY approve if ALL of the following are EXECUTED AND PASS:

1. successCriteria command runs and exits 0 (YOU RAN IT)
2. Test suite passes (YOU RAN IT)
3. Behavioral edge case tests pass (YOU RAN THEM)
4. Root cause verification tests pass (YOU RAN THEM)
5. No new failures in broader test suite (YOU RAN IT)

## 🔴 DEBUGGING METHODOLOGY CHECK

Before approving, verify the worker didn't take shortcuts:

### Ad Hoc Fix Detection

- Did worker fix ONE instance? → Grep for similar patterns.

### Root Cause vs Symptom

- Did worker add a workaround? → Find the ACTUAL bug.

### Lazy Debugging Red Flags (INSTANT REJECT)

- Worker suggests "restart the service" → REJECT
- Worker suggests "clear the cache" → REJECT
- Worker says "works on my machine" → REJECT
- Worker blames the test → REJECT unless PROVEN
```

### Context Strategy

```json
{
  "sources": [
    { "topic": "ISSUE_OPENED", "limit": 1 },
    { "topic": "INVESTIGATION_COMPLETE", "limit": 1 },
    { "topic": "FIX_APPLIED", "since": "last_agent_start", "limit": 1 }
  ],
  "format": "chronological",
  "maxTokens": "{{max_tokens}}"
}
```

### Triggers

| Topic         | Condition | Action       |
| ------------- | --------- | ------------ |
| `FIX_APPLIED` | (none)    | execute_task |

### Hook (onComplete)

```json
{
  "action": "publish_message",
  "config": {
    "topic": "VALIDATION_RESULT",
    "content": {
      "text": "{{result.summary}}",
      "data": {
        "approved": "{{result.approved}}",
        "errors": "{{result.errors}}",
        "testResults": "{{result.testResults}}"
      }
    }
  }
}
```

---

## Key Differences from Full Workflow

| Aspect          | Full Workflow                 | Debug Workflow                |
| --------------- | ----------------------------- | ----------------------------- |
| Planning        | Creates new feature plan      | Investigates existing bugs    |
| Root causes     | N/A                           | Must enumerate ALL causes     |
| Similarity scan | N/A                           | MANDATORY for all bugs        |
| Implementation  | Execute plan steps            | Fix specific root causes      |
| Validation      | Check acceptance criteria     | Behavioral execution testing  |
| Evidence format | criteriaResults with evidence | commandResult with exit codes |
