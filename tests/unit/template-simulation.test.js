const assert = require('node:assert');

const {
  simulateConsensusGates,
} = require('../../src/template-validation/simulate-consensus-gates');
const { SHARED_TRIGGER_SCRIPT } = require('../../src/agents/git-pusher-template');

describe('Template micro-simulation (consensus gates)', function () {
  it('flags consensus gates that fire on duplicate sender', function () {
    const config = {
      name: 'Bad consensus template',
      agents: [
        {
          id: 'validator-a',
          role: 'validator',
          triggers: [{ topic: 'START', action: 'execute_task' }],
          hooks: { onComplete: { action: 'publish_message', config: { topic: 'X' } } },
        },
        {
          id: 'validator-b',
          role: 'validator',
          triggers: [{ topic: 'START', action: 'execute_task' }],
          hooks: { onComplete: { action: 'publish_message', config: { topic: 'X' } } },
        },
        {
          id: 'consensus-coordinator',
          role: 'coordinator',
          triggers: [
            {
              topic: 'X',
              logic: {
                engine: 'javascript',
                // BUGGY: counts messages, doesn't require distinct senders.
                script:
                  "const results = ledger.query({ topic: 'X' }); return results.length === 2;",
              },
              action: 'execute_task',
            },
          ],
        },
      ],
    };

    const failures = simulateConsensusGates(config);
    assert.ok(failures.length >= 1);
    assert.ok(failures.some((f) => f.includes('fires early')));
  });

  it('accepts consensus gates that require distinct producers via helpers.allResponded', function () {
    const config = {
      name: 'Good consensus template',
      agents: [
        {
          id: 'validator-a',
          role: 'validator',
          triggers: [{ topic: 'START', action: 'execute_task' }],
          hooks: { onComplete: { action: 'publish_message', config: { topic: 'X' } } },
        },
        {
          id: 'validator-b',
          role: 'validator',
          triggers: [{ topic: 'START', action: 'execute_task' }],
          hooks: { onComplete: { action: 'publish_message', config: { topic: 'X' } } },
        },
        {
          id: 'consensus-coordinator',
          role: 'coordinator',
          triggers: [
            {
              topic: 'X',
              logic: {
                engine: 'javascript',
                script: "return helpers.allResponded(['validator-a','validator-b'], 'X', 0);",
              },
              action: 'execute_task',
            },
          ],
        },
      ],
    };

    const failures = simulateConsensusGates(config);
    assert.deepStrictEqual(failures, []);
  });

  it('flags completion handlers that depend on a stage-start topic nobody publishes', function () {
    const config = {
      name: 'Broken debug PR topology',
      agents: [
        {
          id: 'fixer',
          role: 'implementation',
          triggers: [{ topic: 'INVESTIGATION_COMPLETE', action: 'execute_task' }],
          hooks: { onComplete: { action: 'publish_message', config: { topic: 'FIX_APPLIED' } } },
        },
        {
          id: 'tester',
          role: 'validator',
          triggers: [{ topic: 'FIX_APPLIED', action: 'execute_task' }],
          hooks: {
            onComplete: { action: 'publish_message', config: { topic: 'VALIDATION_RESULT' } },
          },
        },
        {
          id: 'git-pusher',
          role: 'completion-detector',
          triggers: [
            {
              topic: 'VALIDATION_RESULT',
              logic: {
                engine: 'javascript',
                script: SHARED_TRIGGER_SCRIPT,
              },
              action: 'execute_task',
            },
          ],
        },
      ],
    };

    const failures = simulateConsensusGates(config);

    assert.ok(
      failures.some((failure) =>
        failure.includes('depends on missing stage topic(s): IMPLEMENTATION_READY')
      ),
      failures.join('\n')
    );
  });
});
