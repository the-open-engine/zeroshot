/**
 * Tests for CANNOT_VALIDATE and CANNOT_VALIDATE_YET statuses in criteriaResults
 *
 * CANNOT_VALIDATE (permanent):
 * - Used when verification is impossible due to environment (missing tools, no access)
 * - Treated as PASS (we accept we can't check it)
 * - Skipped on retry iterations (environment won't change)
 * - Displayed as warning (yellow) in CLI and export
 *
 * CANNOT_VALIDATE_YET (temporary):
 * - Used when verification is not yet possible (work incomplete, tests failing)
 * - Treated as FAIL (work needs to continue)
 * - Re-evaluated on retry iterations (condition may have changed)
 * - Displayed as error (red) in CLI and export
 */

const assert = require('assert');
const Orchestrator = require('../src/orchestrator.js');
const fs = require('fs');
const os = require('os');
const path = require('path');

// Isolate tests from user settings
const testSettingsDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-test-settings-'));
const testSettingsFile = path.join(testSettingsDir, 'settings.json');
fs.writeFileSync(testSettingsFile, JSON.stringify({ maxModel: 'opus', minModel: null }));
process.env.ZEROSHOT_SETTINGS_FILE = testSettingsFile;

const { buildContext } = require('../src/agent/agent-context-builder');

const createTestOrchestrator = () => {
  const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-test-'));
  const orchestrator = new Orchestrator({ quiet: true, skipLoad: true, stateDir: tmpDir });
  return { orchestrator, tmpDir };
};

const cleanupTmpDir = (tmpDir) => {
  if (fs.existsSync(tmpDir)) {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  }
};

const baseContextParams = (overrides = {}) => ({
  id: 'validator',
  role: 'validator',
  iteration: 2,
  config: { contextStrategy: { sources: [] } },
  messageBus: { query: () => [] },
  cluster: { id: 'test-cluster', createdAt: Date.now() - 60000 },
  triggeringMessage: { topic: 'IMPLEMENTATION_READY', sender: 'worker' },
  ...overrides,
});

const mockBusWithCriteria = (criteriaResults) => ({
  query: ({ topic }) => {
    if (topic === 'VALIDATION_RESULT') {
      return [{ content: { data: { criteriaResults } } }];
    }
    return [];
  },
});

describe('CANNOT_VALIDATE Export Markdown', function () {
  this.timeout(5000);

  let orchestrator;
  let tmpDir;

  beforeEach(() => {
    ({ orchestrator, tmpDir } = createTestOrchestrator());
  });

  afterEach(() => {
    cleanupTmpDir(tmpDir);
  });

  it('should show CANNOT_VALIDATE criteria as warnings in export', function () {
    const mockCluster = {
      state: 'completed',
      createdAt: Date.now() - 60000,
      agents: [{ id: 'worker' }, { id: 'validator' }],
    };

    const messages = [
      {
        id: 'msg-1',
        topic: 'ISSUE_OPENED',
        sender: 'user',
        timestamp: Date.now() - 10000,
        content: { text: 'Test issue' },
      },
      {
        id: 'msg-2',
        topic: 'VALIDATION_RESULT',
        sender: 'validator-requirements',
        timestamp: Date.now(),
        content: {
          text: 'Validation complete with caveats',
          data: {
            approved: true,
            summary: 'Implementation approved with manual verification needed',
            criteriaResults: [
              {
                id: 'AC1',
                status: 'PASS',
                evidence: { command: 'npm test', exitCode: 0, output: 'All tests passed' },
              },
              {
                id: 'AC2',
                status: 'CANNOT_VALIDATE',
                reason: 'kubectl not installed - cannot verify K8s deployment',
              },
              {
                id: 'AC3',
                status: 'CANNOT_VALIDATE',
                reason: 'No SSH access to production server',
              },
            ],
          },
        },
      },
    ];

    const markdown = orchestrator._exportMarkdown(mockCluster, 'test-cluster', messages);

    assert.ok(
      markdown.includes('Could Not Validate'),
      'Should include "Could Not Validate" section'
    );
    assert.ok(markdown.includes('2 criteria'), 'Should show count of CANNOT_VALIDATE criteria');
    assert.ok(markdown.includes('AC2'), 'Should include AC2 criterion ID');
    assert.ok(markdown.includes('kubectl not installed'), 'Should include AC2 reason');
    assert.ok(markdown.includes('AC3'), 'Should include AC3 criterion ID');
    assert.ok(markdown.includes('No SSH access'), 'Should include AC3 reason');
  });

  it('should not show CANNOT_VALIDATE section when all criteria pass', function () {
    const mockCluster = {
      state: 'completed',
      createdAt: Date.now() - 60000,
      agents: [{ id: 'validator' }],
    };

    const messages = [
      {
        id: 'msg-1',
        topic: 'ISSUE_OPENED',
        sender: 'user',
        timestamp: Date.now() - 10000,
        content: { text: 'Test issue' },
      },
      {
        id: 'msg-2',
        topic: 'VALIDATION_RESULT',
        sender: 'validator',
        timestamp: Date.now(),
        content: {
          data: {
            approved: true,
            summary: 'All criteria passed',
            criteriaResults: [
              { id: 'AC1', status: 'PASS', evidence: {} },
              { id: 'AC2', status: 'PASS', evidence: {} },
            ],
          },
        },
      },
    ];

    const markdown = orchestrator._exportMarkdown(mockCluster, 'test-cluster', messages);

    assert.ok(
      !markdown.includes('Could Not Validate'),
      'Should not include CANNOT_VALIDATE section when all pass'
    );
  });

  it('should handle missing reason gracefully', function () {
    const mockCluster = {
      state: 'completed',
      createdAt: Date.now() - 60000,
      agents: [{ id: 'validator' }],
    };

    const messages = [
      {
        id: 'msg-1',
        topic: 'ISSUE_OPENED',
        sender: 'user',
        timestamp: Date.now() - 10000,
        content: { text: 'Test issue' },
      },
      {
        id: 'msg-2',
        topic: 'VALIDATION_RESULT',
        sender: 'validator',
        timestamp: Date.now(),
        content: {
          data: {
            approved: true,
            criteriaResults: [{ id: 'AC1', status: 'CANNOT_VALIDATE' }],
          },
        },
      },
    ];

    const markdown = orchestrator._exportMarkdown(mockCluster, 'test-cluster', messages);

    assert.ok(
      markdown.includes('No reason provided'),
      'Should show fallback text when reason missing'
    );
  });
});

describe('CANNOT_VALIDATE Schema Validation', function () {
  it('should accept CANNOT_VALIDATE as valid status in criteriaResults', function () {
    const validCriteriaResult = {
      id: 'AC1',
      status: 'CANNOT_VALIDATE',
      reason: 'Tool not available',
    };

    assert.strictEqual(validCriteriaResult.status, 'CANNOT_VALIDATE');
    assert.ok(validCriteriaResult.reason, 'Should have reason field');
  });

  it('should accept CANNOT_VALIDATE_YET as valid status in criteriaResults', function () {
    const validCriteriaResult = {
      id: 'AC1',
      status: 'CANNOT_VALIDATE_YET',
      reason: 'Tests not passing yet - refactor incomplete',
    };

    assert.strictEqual(validCriteriaResult.status, 'CANNOT_VALIDATE_YET');
    assert.ok(validCriteriaResult.reason, 'Should have reason field');
  });
});

describe('CANNOT_VALIDATE Context Builder - Core Behavior', function () {
  it('should inject skip section with ALL CANNOT_VALIDATE criteria', function () {
    const criteria = [
      { id: 'AC1', status: 'PASS', evidence: {} },
      { id: 'AC2', status: 'CANNOT_VALIDATE', reason: 'kubectl not installed' },
      { id: 'AC3', status: 'CANNOT_VALIDATE', reason: 'No SSH access to prod' },
      { id: 'AC4', status: 'FAIL', reason: 'Tests failed' },
    ];

    const context = buildContext(baseContextParams({ messageBus: mockBusWithCriteria(criteria) }));

    assert.ok(context.includes('Permanently Unverifiable Criteria'), 'Missing header');
    assert.ok(context.includes('AC2'), 'Missing AC2');
    assert.ok(context.includes('kubectl not installed'), 'Missing AC2 reason');
    assert.ok(context.includes('AC3'), 'Missing AC3');
    assert.ok(context.includes('No SSH access'), 'Missing AC3 reason');
    assert.ok(!context.includes('**AC1**'), 'Should not include PASS criteria');
    assert.ok(!context.includes('**AC4**'), 'Should not include FAIL criteria');
    assert.ok(context.includes('Do NOT re-attempt'), 'Missing skip instruction');
  });

  it('should NOT inject skip section for non-validator roles', function () {
    const criteria = [{ id: 'AC1', status: 'CANNOT_VALIDATE', reason: 'test' }];

    for (const role of ['implementation', 'worker', 'planner', 'tester', 'conductor']) {
      const context = buildContext(
        baseContextParams({
          role,
          messageBus: mockBusWithCriteria(criteria),
        })
      );
      assert.ok(
        !context.includes('Permanently Unverifiable Criteria'),
        `Should NOT inject for role="${role}"`
      );
    }
  });

  it('should deduplicate criteria across multiple validation results', function () {
    const messageBus = {
      query: ({ topic }) => {
        if (topic === 'VALIDATION_RESULT') {
          return [
            {
              content: {
                data: {
                  criteriaResults: [{ id: 'AC1', status: 'CANNOT_VALIDATE', reason: 'R1' }],
                },
              },
            },
            {
              content: {
                data: {
                  criteriaResults: [{ id: 'AC1', status: 'CANNOT_VALIDATE', reason: 'R1' }],
                },
              },
            },
            {
              content: {
                data: {
                  criteriaResults: [{ id: 'AC1', status: 'CANNOT_VALIDATE', reason: 'R1' }],
                },
              },
            },
          ];
        }
        return [];
      },
    };

    const context = buildContext(baseContextParams({ messageBus }));

    const matches = context.match(/\*\*AC1\*\*/g) || [];
    assert.strictEqual(matches.length, 1, `AC1 appeared ${matches.length} times, expected 1`);
  });
});

describe('CANNOT_VALIDATE Context Builder - Malformed Data', function () {
  it('should handle null criteriaResults gracefully', function () {
    const messageBus = {
      query: ({ topic }) => {
        if (topic === 'VALIDATION_RESULT') {
          return [{ content: { data: { criteriaResults: null } } }];
        }
        return [];
      },
    };

    const context = buildContext(baseContextParams({ messageBus }));
    assert.ok(!context.includes('Previously Unverifiable'), 'Should not inject with null criteria');
  });

  it('should handle undefined criteriaResults gracefully', function () {
    const messageBus = {
      query: ({ topic }) => {
        if (topic === 'VALIDATION_RESULT') {
          return [{ content: { data: {} } }];
        }
        return [];
      },
    };

    const context = buildContext(baseContextParams({ messageBus }));
    assert.ok(
      !context.includes('Previously Unverifiable'),
      'Should not inject with undefined criteria'
    );
  });

  it('should handle missing content.data gracefully', function () {
    const messageBus = {
      query: ({ topic }) => {
        if (topic === 'VALIDATION_RESULT') {
          return [{ content: {} }, { content: null }, {}];
        }
        return [];
      },
    };

    const context = buildContext(baseContextParams({ messageBus }));
    assert.ok(typeof context === 'string', 'Should return valid context');
  });

  it('should handle criteriaResults with missing id field', function () {
    const criteria = [
      { status: 'CANNOT_VALIDATE', reason: 'test' },
      { id: 'AC1', status: 'CANNOT_VALIDATE', reason: 'valid' },
    ];

    const context = buildContext(baseContextParams({ messageBus: mockBusWithCriteria(criteria) }));
    assert.ok(context.includes('AC1'), 'Should include valid criterion');
  });

  it('should handle criteriaResults with missing reason field', function () {
    const criteria = [{ id: 'AC1', status: 'CANNOT_VALIDATE' }];

    const context = buildContext(baseContextParams({ messageBus: mockBusWithCriteria(criteria) }));

    assert.ok(context.includes('AC1'), 'Should include criterion');
    assert.ok(context.includes('No reason provided'), 'Should use fallback reason');
  });

  it('should handle empty criteriaResults array', function () {
    const context = buildContext(baseContextParams({ messageBus: mockBusWithCriteria([]) }));

    assert.ok(!context.includes('Previously Unverifiable'), 'Should not inject with empty array');
  });

  it('should handle criteriaResults that is not an array', function () {
    const messageBus = {
      query: ({ topic }) => {
        if (topic === 'VALIDATION_RESULT') {
          return [{ content: { data: { criteriaResults: 'not-an-array' } } }];
        }
        return [];
      },
    };

    const context = buildContext(baseContextParams({ messageBus }));
    assert.ok(!context.includes('Previously Unverifiable'), 'Should not inject with non-array');
  });
});

describe('CANNOT_VALIDATE Context Builder - Message Bus Behavior', function () {
  it('should handle empty message bus results', function () {
    const messageBus = { query: () => [] };

    const context = buildContext(baseContextParams({ messageBus }));
    assert.ok(!context.includes('Previously Unverifiable'), 'Should not inject with no messages');
  });

  it('should only extract from VALIDATION_RESULT topic', function () {
    let queriedTopics = [];
    const messageBus = {
      query: ({ topic }) => {
        queriedTopics.push(topic);
        return [];
      },
    };

    buildContext(baseContextParams({ messageBus }));

    assert.ok(queriedTopics.includes('VALIDATION_RESULT'), 'Should query VALIDATION_RESULT');
  });

  it('should use cluster.createdAt as since timestamp', function () {
    let capturedSince = null;
    const createdAt = Date.now() - 120000;
    const messageBus = {
      query: ({ since }) => {
        capturedSince = since;
        return [];
      },
    };

    buildContext(baseContextParams({ messageBus, cluster: { id: 'test', createdAt } }));

    assert.strictEqual(capturedSince, createdAt, 'Should use cluster createdAt');
  });
});

describe('CANNOT_VALIDATE Context Builder - Iteration Behavior', function () {
  it('should inject on iteration 1 if CANNOT_VALIDATE exists from iteration 0', function () {
    const criteria = [{ id: 'AC1', status: 'CANNOT_VALIDATE', reason: 'No kubectl' }];

    const context = buildContext(
      baseContextParams({
        iteration: 1,
        messageBus: mockBusWithCriteria(criteria),
      })
    );

    assert.ok(context.includes('AC1'), 'Should inject on iteration 1');
  });

  it('should accumulate CANNOT_VALIDATE across iterations', function () {
    const messageBus = {
      query: ({ topic }) => {
        if (topic === 'VALIDATION_RESULT') {
          return [
            {
              content: {
                data: {
                  criteriaResults: [{ id: 'AC1', status: 'CANNOT_VALIDATE', reason: 'R1' }],
                },
              },
            },
            {
              content: {
                data: {
                  criteriaResults: [{ id: 'AC2', status: 'CANNOT_VALIDATE', reason: 'R2' }],
                },
              },
            },
          ];
        }
        return [];
      },
    };

    const context = buildContext(baseContextParams({ iteration: 3, messageBus }));

    assert.ok(context.includes('AC1'), 'Should include AC1 from iteration 1');
    assert.ok(context.includes('AC2'), 'Should include AC2 from iteration 2');
  });
});

describe('CANNOT_VALIDATE_YET Export Markdown', function () {
  this.timeout(5000);

  let orchestrator;
  let tmpDir;

  beforeEach(() => {
    ({ orchestrator, tmpDir } = createTestOrchestrator());
  });

  afterEach(() => {
    cleanupTmpDir(tmpDir);
  });

  it('should show CANNOT_VALIDATE_YET as errors (not warnings) in export', function () {
    const mockCluster = {
      state: 'completed',
      createdAt: Date.now() - 60000,
      agents: [{ id: 'validator' }],
    };

    const messages = [
      {
        id: 'msg-1',
        topic: 'ISSUE_OPENED',
        sender: 'user',
        timestamp: Date.now() - 10000,
        content: { text: 'Test issue' },
      },
      {
        id: 'msg-2',
        topic: 'VALIDATION_RESULT',
        sender: 'validator',
        timestamp: Date.now(),
        content: {
          data: {
            approved: false,
            summary: 'Work incomplete',
            criteriaResults: [
              { id: 'AC1', status: 'PASS', evidence: {} },
              {
                id: 'AC2',
                status: 'CANNOT_VALIDATE_YET',
                reason: 'Tests not passing - 204 warnings remain',
              },
            ],
          },
        },
      },
    ];

    const markdown = orchestrator._exportMarkdown(mockCluster, 'test-cluster', messages);

    assert.ok(
      markdown.includes('Cannot Validate Yet'),
      'Should include "Cannot Validate Yet" section'
    );
    assert.ok(markdown.includes('work incomplete'), 'Should indicate work incomplete');
    assert.ok(markdown.includes('AC2'), 'Should include AC2 criterion ID');
    assert.ok(markdown.includes('204 warnings'), 'Should include AC2 reason');
  });

  it('should show both CANNOT_VALIDATE and CANNOT_VALIDATE_YET separately', function () {
    const mockCluster = {
      state: 'completed',
      createdAt: Date.now() - 60000,
      agents: [{ id: 'validator' }],
    };

    const messages = [
      {
        id: 'msg-1',
        topic: 'ISSUE_OPENED',
        sender: 'user',
        timestamp: Date.now() - 10000,
        content: { text: 'Test issue' },
      },
      {
        id: 'msg-2',
        topic: 'VALIDATION_RESULT',
        sender: 'validator',
        timestamp: Date.now(),
        content: {
          data: {
            approved: false,
            criteriaResults: [
              { id: 'AC1', status: 'CANNOT_VALIDATE', reason: 'kubectl not installed' },
              { id: 'AC2', status: 'CANNOT_VALIDATE_YET', reason: 'Refactor incomplete' },
            ],
          },
        },
      },
    ];

    const markdown = orchestrator._exportMarkdown(mockCluster, 'test-cluster', messages);

    assert.ok(markdown.includes('Could Not Validate'), 'Should have permanent section');
    assert.ok(markdown.includes('Cannot Validate Yet'), 'Should have temporary section');
    assert.ok(markdown.includes('AC1'), 'Should include AC1');
    assert.ok(markdown.includes('AC2'), 'Should include AC2');
    assert.ok(markdown.includes('kubectl'), 'Should include AC1 reason');
    assert.ok(markdown.includes('Refactor'), 'Should include AC2 reason');
  });
});

describe('CANNOT_VALIDATE_YET Context Builder - Not Skipped on Retry', function () {
  it('should NOT inject skip instructions for CANNOT_VALIDATE_YET criteria', function () {
    const criteria = [
      { id: 'AC1', status: 'CANNOT_VALIDATE_YET', reason: 'Tests failing' },
      { id: 'AC2', status: 'CANNOT_VALIDATE_YET', reason: 'Build broken' },
    ];

    const context = buildContext(baseContextParams({ messageBus: mockBusWithCriteria(criteria) }));

    assert.ok(
      !context.includes('Permanently Unverifiable'),
      'Should NOT have skip section for CANNOT_VALIDATE_YET'
    );
    assert.ok(
      !context.includes('SKIP THESE'),
      'Should NOT have skip instruction for CANNOT_VALIDATE_YET'
    );
  });

  it('should only skip CANNOT_VALIDATE (permanent), not CANNOT_VALIDATE_YET (temporary)', function () {
    const criteria = [
      { id: 'AC1', status: 'CANNOT_VALIDATE', reason: 'kubectl not installed' },
      { id: 'AC2', status: 'CANNOT_VALIDATE_YET', reason: 'Refactor incomplete' },
    ];

    const context = buildContext(baseContextParams({ messageBus: mockBusWithCriteria(criteria) }));

    assert.ok(context.includes('AC1'), 'AC1 should be in skip section');
    assert.ok(context.includes('kubectl not installed'), 'AC1 reason should be in skip section');

    const skipSection = context.match(/Permanently Unverifiable Criteria[\s\S]*?(?=\n## |$)/)?.[0];
    if (skipSection) {
      assert.ok(!skipSection.includes('AC2'), 'AC2 should NOT be in skip section');
      assert.ok(
        !skipSection.includes('Refactor incomplete'),
        'AC2 reason should NOT be in skip section'
      );
    }
  });

  it('should re-evaluate CANNOT_VALIDATE_YET on each iteration', function () {
    const messageBus = {
      query: ({ topic }) => {
        if (topic === 'VALIDATION_RESULT') {
          return [
            {
              content: {
                data: {
                  criteriaResults: [
                    { id: 'AC1', status: 'CANNOT_VALIDATE_YET', reason: 'Build failing' },
                  ],
                },
              },
            },
          ];
        }
        return [];
      },
    };

    const context = buildContext(baseContextParams({ iteration: 3, messageBus }));

    assert.ok(
      !context.includes('AC1') || !context.includes('Permanently Unverifiable'),
      'CANNOT_VALIDATE_YET should never be skipped'
    );
  });
});

// Cleanup settings file
after(() => {
  if (fs.existsSync(testSettingsDir)) {
    fs.rmSync(testSettingsDir, { recursive: true, force: true });
  }
});
