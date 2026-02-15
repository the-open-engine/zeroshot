# Test Quality Validation Workflow

This document describes Zeroshot's test quality enforcement system introduced in v5.4.0.

## Overview

Zeroshot enforces test quality through antipattern detection in validators. Tests must verify behavior, not just pass for the wrong reasons.

## Workflow Stages

### 1. Planning Phase

The planner creates acceptance criteria with explicit verification steps:

```json
{
  "acceptanceCriteria": [
    {
      "id": "AC1",
      "criterion": "API endpoint returns correct status codes",
      "verification": "curl -X POST /api/users -d '{invalid}' → 400 + {error: 'Invalid input'}",
      "priority": "MUST"
    },
    {
      "id": "AC2",
      "criterion": "Tests pass with coverage >80%",
      "verification": "npm test -- --coverage → all pass, coverage report shows >80%",
      "priority": "MUST"
    }
  ]
}
```

**Required Fields:**
- `id`: Unique identifier (AC1, AC2, etc.)
- `criterion`: Testable statement (no vague claims)
- `verification`: Exact command or steps to verify
- `priority`: MUST (blocks) / SHOULD (important) / NICE (bonus)

**Minimum 3 criteria required** (enforced via JSON schema).

### 2. Implementation Phase

Worker must:
- Implement all features
- Write tests for new functionality
- Run tests locally before submitting
- Address all MUST criteria

### 3. Validation Phase

#### Validator: Requirements

Checks acceptance criteria compliance:

1. Parse `acceptanceCriteria` from PLAN_READY
2. For each criterion: run verification command
3. Record evidence (command + output)
4. Reject if any MUST criterion fails

#### Validator: Tester

Enforces test quality and execution:

1. **RUN THE TESTS** (not just read them)
   - Execute test command: `npm test`, `pytest`, etc.
   - Record full output in `testResults` field
2. **Detect Antipatterns**
3. **Verify Coverage** (if specified in criteria)
4. **Reject if tests fail or quality is poor**

## Antipattern Detection

### 1. Verification Theater

**Problem:** Tests check existence without verifying correctness.

```javascript
// ❌ REJECTED
test('user creation works', () => {
  const user = createUser();
  expect(user).toBeDefined();
  expect(user.id).toBeTruthy();
});
```

```javascript
// ✅ APPROVED
test('user creation works', () => {
  const user = createUser({ email: 'test@example.com', role: 'admin' });
  expect(user.id).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}/); // UUID format
  expect(user.email).toBe('test@example.com');
  expect(user.role).toBe('admin');
  expect(user.createdAt).toBeInstanceOf(Date);
});
```

### 2. Mocking Expected Results

**Problem:** Circular testing - mock returns the exact value being asserted.

```javascript
// ❌ REJECTED
test('API call returns user', async () => {
  const expectedUser = { id: 1, name: 'Alice' };
  mockApi.getUser.mockResolvedValue(expectedUser);

  const result = await fetchUser(1);
  expect(result).toEqual(expectedUser); // Circular!
});
```

```javascript
// ✅ APPROVED
test('API call returns user', async () => {
  mockApi.getUser.mockResolvedValue({ id: 1, name: 'Alice', role: 'admin' });

  const result = await fetchUser(1);
  expect(result.id).toBe(1);
  expect(result.displayName).toBe('Alice (Admin)'); // Tests actual transformation
  expect(result.canEdit).toBe(true); // Tests derived property
});
```

### 3. Timing Dependencies

**Problem:** Arbitrary sleeps make tests flaky.

```javascript
// ❌ REJECTED
test('async operation completes', async () => {
  startAsyncOperation();
  await sleep(1000);
  expect(operationComplete).toBe(true);
});
```

```javascript
// ✅ APPROVED
test('async operation completes', async () => {
  const promise = startAsyncOperation();
  await promise; // Proper synchronization
  expect(operationComplete).toBe(true);
});

// Or with proper waiting
test('async operation completes', async () => {
  startAsyncOperation();
  await waitFor(() => operationComplete, { timeout: 5000 });
  expect(operationComplete).toBe(true);
});
```

### 4. Missing Isolation

**Problem:** Tests share state or make real network calls.

```javascript
// ❌ REJECTED
let sharedUser;

test('creates user', () => {
  sharedUser = createUser();
  expect(sharedUser).toBeDefined();
});

test('updates user', () => {
  updateUser(sharedUser.id, { name: 'Bob' }); // Depends on previous test!
  expect(sharedUser.name).toBe('Bob');
});
```

```javascript
// ✅ APPROVED
describe('User operations', () => {
  let user;

  beforeEach(() => {
    user = createUser(); // Fresh state each test
  });

  test('creates user', () => {
    expect(user.id).toBeDefined();
  });

  test('updates user', () => {
    const updated = updateUser(user.id, { name: 'Bob' });
    expect(updated.name).toBe('Bob');
  });
});
```

## Validator Output Format

### Requirements Validator

```json
{
  "approved": false,
  "summary": "AC1 failed: missing input validation",
  "errors": ["POST /api/users with invalid email → 500 instead of 400"],
  "criteriaResults": [
    {
      "id": "AC1",
      "status": "FAIL",
      "evidence": {
        "command": "curl -X POST /api/users -d '{\"email\":\"invalid\"}'",
        "exitCode": 0,
        "output": "{\"error\":\"Internal server error\"}"
      }
    }
  ]
}
```

### Tester Validator

```json
{
  "approved": false,
  "summary": "Tests fail: 3/15 failing",
  "errors": [
    "Test 'user creation' fails: verification theater (only checks toBeDefined)",
    "Test 'API call' fails: timing dependency (arbitrary sleep)"
  ],
  "testResults": "FAIL tests/user.test.js\n  ✓ creates user (12ms)\n  ✗ updates user (timeout)\n\n3 failed, 12 passed, 15 total"
}
```

## Implementation References

### Planner Acceptance Criteria Schema

See `cluster-templates/base-templates/full-workflow.json` lines 79-107:

```json
{
  "acceptanceCriteria": {
    "type": "array",
    "description": "EXPLICIT, TESTABLE acceptance criteria. Each must be verifiable. NO VAGUE BULLSHIT.",
    "items": {
      "type": "object",
      "properties": {
        "id": { "type": "string" },
        "criterion": { "type": "string" },
        "verification": { "type": "string" },
        "priority": { "enum": ["MUST", "SHOULD", "NICE"] }
      }
    },
    "minItems": 3
  }
}
```

### Validator-Tester Prompt

See `cluster-templates/base-templates/full-workflow.json` lines 665-667 for full enforcement rules:

- RUN tests (not read)
- Record actual output
- Detect antipatterns
- Reject weak tests

## Benefits

1. **Catches False Positives**: Tests that pass but don't verify behavior
2. **Forces Quality**: Workers must write meaningful tests
3. **Reproducible**: Validators execute tests, not just read code
4. **Non-Negotiable**: Validation failure blocks completion

## Related Files

- `cluster-templates/base-templates/full-workflow.json`: Validator prompts and schemas
- `AGENTS.md`: Behavioral rules and antipattern table
- `CLAUDE.md`: Quick reference for test quality requirements
- `src/isolation-manager.js`: Projects/ subdirectory fix (issue #2)
