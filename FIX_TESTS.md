# Test Failures Snapshot

## Overall

- 853 passing (24m)
- 16 pending
- 15 failing

## Failing Tests (15)

1. MessageBus Integration
   - Suite: Batch Publishing (Atomic)
   - Test: should prevent interleaving with concurrent agents
   - Error: AssertionError [ERR_ASSERTION]: Agent A's messages should be contiguous
   - Expected vs actual: 1 expected, 2 actual
   - Location: tests/integration/message-bus.test.js:441:14
   - Status: Fixed (verified locally)

2. npm install retry logic
   - Suite: Retry mechanism
   - Test: retries npm install on failure with exponential backoff
   - Error: AssertionError [ERR_ASSERTION]: Should retry twice after initial failure
   - Expected vs actual: 3 expected, 2 actual
   - Location: tests/integration/npm-install-retry.test.js:110:14
   - Status: Fixed (pending verification)

3. npm install retry logic
   - Suite: Retry mechanism
   - Test: fails after max retries exceeded
   - Error: AssertionError [ERR_ASSERTION]: Should attempt 3 times total
   - Expected vs actual: 3 expected, 2 actual
   - Location: tests/integration/npm-install-retry.test.js:148:14
   - Status: Fixed (pending verification)

4. npm install retry logic
   - Suite: Retry mechanism
   - Test: does not retry if first attempt succeeds
   - Error: AssertionError [ERR_ASSERTION]: Should only attempt once on success
   - Expected vs actual: 1 expected, 4 actual
   - Location: tests/integration/npm-install-retry.test.js:183:14
   - Status: Fixed (pending verification)

5. npm install retry logic
   - Suite: Retry mechanism
   - Test: handles execution errors during retry
   - Error: AssertionError [ERR_ASSERTION]: Should retry after execution errors
   - Expected vs actual: 3 expected, 1 actual
   - Location: tests/integration/npm-install-retry.test.js:249:14
   - Status: Fixed (pending verification)

6. npm install retry logic
   - Suite: Exponential backoff timing
   - Test: uses correct delay calculation: 2s, 4s, 8s
   - Error: AssertionError [ERR_ASSERTION]: Should have 3 attempts
   - Expected vs actual: 3 expected, 2 actual
   - Location: tests/integration/npm-install-retry.test.js:291:14
   - Status: Fixed (pending verification)

7. npm install retry logic
   - Suite: Non-fatal failure behavior
   - Test: logs warning but continues when all retries fail
   - Error: AssertionError [ERR_ASSERTION]: Should log warnings about npm install failures
   - Expected vs actual: true expected, false actual
   - Location: tests/integration/npm-install-retry.test.js:343:9
   - Status: Fixed (pending verification)

8. Orchestrator Isolation Mode Integration
   - Suite: Container Lifecycle
   - Test: should preserve workspace on stop for resume capability
   - Error: AssertionError [ERR_ASSERTION]: Isolated workspace should be PRESERVED for resume: /tmp/zeroshot-isolated/flaming-surge-53
   - Expected vs actual: true expected, false actual
   - Location: tests/integration/orchestrator-isolation.test.js:250:7
   - Status: Fixed (pending verification)

9. Orchestrator Isolation Mode Integration
   - Suite: Resume Capability
   - Test: should recreate container on resume using preserved workspace
   - Error: AssertionError [ERR_ASSERTION]: Workspace should be preserved for resume
   - Expected vs actual: true expected, false actual
   - Location: tests/integration/orchestrator-isolation.test.js:451:7
   - Status: Fixed (pending verification)

10. Orchestrator Isolation Mode Integration
    - Suite: Resume Capability
    - Test: should not be resumable after kill (cluster removed)
    - Error: AssertionError [ERR_ASSERTION]: Workspace should exist before kill
    - Expected vs actual: true expected, false actual
    - Location: tests/integration/orchestrator-isolation.test.js:547:7
    - Status: Fixed (pending verification)

11. Orchestrator Isolation Mode Integration
    - Suite: Container Lifecycle
    - Test: should preserve workspace on stop for resume capability
    - Error: AssertionError [ERR_ASSERTION]: Isolated workspace should be PRESERVED for resume: /tmp/zeroshot-isolated/sonic-glyph-68
    - Expected vs actual: true expected, false actual
    - Location: tests/integration/slow/orchestrator-isolation.test.js:253:7
    - Status: Fixed (pending verification)

12. Orchestrator Isolation Mode Integration
    - Suite: Resume Capability
    - Test: should recreate container on resume using preserved workspace
    - Error: AssertionError [ERR_ASSERTION]: Workspace should be preserved for resume
    - Expected vs actual: true expected, false actual
    - Location: tests/integration/slow/orchestrator-isolation.test.js:454:7
    - Status: Fixed (pending verification)

13. Orchestrator Isolation Mode Integration
    - Suite: Resume Capability
    - Test: should not be resumable after kill (cluster removed)
    - Error: AssertionError [ERR_ASSERTION]: Workspace should exist before kill
    - Expected vs actual: true expected, false actual
    - Location: tests/integration/slow/orchestrator-isolation.test.js:550:7
    - Status: Fixed (pending verification)

14. Isolated Mode Output Capture
    - Test: should read agent output from log file, not spawn stdout
    - Error: Timeout of 150000ms exceeded. For async tests and hooks, ensure "done()" is called; if returning a Promise, ensure it resolves.
    - Location: tests/unit/isolated-mode-output-capture.test.js
    - Status: Fixed (pending verification)

15. Isolated Mode Output Capture
    - Hook: "after each" hook for "should read agent output from log file, not spawn stdout"
    - Error: Timeout of 2000ms exceeded. For async tests and hooks, ensure "done()" is called; if returning a Promise, ensure it resolves.
    - Location: tests/unit/isolated-mode-output-capture.test.js
    - Status: Fixed (pending verification)
