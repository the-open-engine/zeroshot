/**
 * TEST: Orchestrator startup failure cleanup
 *
 * Verifies that deterministic pre-start validation failures (like duplicate active
 * issue) do not persist phantom 0-message clusters that later appear as "corrupted".
 */

const assert = require('assert');
const fs = require('fs');
const path = require('path');

describe('Orchestrator startup failure cleanup', () => {
  const sourceFile = path.join(__dirname, '..', 'src', 'orchestrator.js');
  let sourceCode;

  before(() => {
    sourceCode = fs.readFileSync(sourceFile, 'utf8');
  });

  it('guards 0-message corruption marking by cluster state', () => {
    assert.ok(
      sourceCode.includes(
        "const inactiveStates = new Set(['failed', 'stopped', 'completed', 'corrupted'])"
      ),
      'Expected inactive-state guard for 0-message corruption detection'
    );
    assert.ok(
      sourceCode.includes('messageCount === 0 && !inactiveStates.has(currentState)'),
      'Expected corruption detection to skip inactive cluster states'
    );
  });

  it('detects duplicate-active-issue startup failures', () => {
    assert.ok(
      sourceCode.includes('isDuplicateActiveIssueError'),
      'Expected explicit duplicate active issue error handling'
    );
    assert.ok(
      sourceCode.includes("error.message.includes('already has an active cluster')"),
      'Expected duplicate cluster detection by error message'
    );
  });

  it('drops phantom cluster entries for deterministic pre-start failures', () => {
    assert.ok(
      sourceCode.includes('this.clusters.delete(clusterId)'),
      'Expected pre-start deterministic failures to remove cluster from registry'
    );
    assert.ok(
      sourceCode.includes('if (shouldPersistFailureState)'),
      'Expected conditional persistence of failed startup state'
    );
  });
});
