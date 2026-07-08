const assert = require('assert');
const fs = require('fs');
const os = require('os');
const path = require('path');

const Orchestrator = require('../src/orchestrator');
const MockTaskRunner = require('./helpers/mock-task-runner');

describe('command proofs cluster integration', function () {
  let tempDir;
  let orchestrator;
  let mockRunner;

  beforeEach(() => {
    tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-command-proofs-'));
    mockRunner = new MockTaskRunner();
    orchestrator = new Orchestrator({
      quiet: true,
      storageDir: tempDir,
      taskRunner: mockRunner,
    });
  });

  afterEach(async () => {
    if (orchestrator) {
      for (const cluster of orchestrator.listClusters()) {
        try {
          await orchestrator.kill(cluster.id);
        } catch {
          // Best effort cleanup.
        }
      }
      orchestrator.close();
    }
    fs.rmSync(tempDir, { recursive: true, force: true });
  });

  it('uses issue-body command proofs as cluster-scoped required gates and agent instructions', async function () {
    const config = {
      ship: {
        commandProofs: [
          {
            id: 'repo-lint',
            profile: 'lint-equivalent',
            command: 'npm run lint',
          },
        ],
      },
      agents: [
        {
          id: 'worker',
          role: 'implementation',
          timeout: 0,
          triggers: [{ topic: 'ISSUE_OPENED', action: 'execute_task' }],
          prompt: 'Implement the requested change.',
          hooks: {
            onComplete: {
              action: 'publish_message',
              config: { topic: 'TASK_COMPLETE', content: { text: 'Done' } },
            },
          },
        },
        {
          id: 'validator',
          role: 'validator',
          timeout: 0,
          triggers: [{ topic: 'TASK_COMPLETE', action: 'execute_task' }],
          prompt: 'Validate the change.',
        },
      ],
    };

    const issueText = [
      'Implement opcore change.',
      '',
      '```zeroshot-command-proofs',
      '[',
      '  {',
      '    "id": "opcore-ci",',
      '    "profile": "ci-equivalent",',
      '    "scope": "repo",',
      '    "description": "Opcore local CI",',
      '    "command": "bash ./scripts/ci/run-local-ci-equivalent.sh"',
      '  }',
      ']',
      '```',
    ].join('\n');

    mockRunner.when('worker').returns(JSON.stringify({ summary: 'Done' }));
    mockRunner.when('validator').returns(JSON.stringify({ approved: true }));

    const result = await orchestrator.start(config, { text: issueText });
    const cluster = orchestrator.getCluster(result.id);

    assert.deepStrictEqual(cluster.commandProofs, [
      { id: 'repo-lint', profile: 'lint-equivalent', command: 'npm run lint' },
      {
        id: 'opcore-ci',
        profile: 'ci-equivalent',
        scope: 'repo',
        description: 'Opcore local CI',
        command: 'bash ./scripts/ci/run-local-ci-equivalent.sh',
      },
    ]);
    assert.deepStrictEqual(cluster.requiredQualityGates, [
      {
        id: 'repo-lint',
        profile: 'lint-equivalent',
        command: 'npm run lint',
        commandProof: true,
      },
      {
        id: 'opcore-ci',
        profile: 'ci-equivalent',
        scope: 'repo',
        description: 'Opcore local CI',
        command: 'bash ./scripts/ci/run-local-ci-equivalent.sh',
        commandProof: true,
      },
    ]);

    const workerContext = mockRunner.getCalls('worker')[0].context;
    assert.match(workerContext, /Reusable Command Proofs/);
    assert.match(workerContext, /zeroshot cmdproof check repo-lint/);
    assert.match(workerContext, /zeroshot cmdproof check opcore-ci/);
    assert.match(workerContext, /bash \.\/scripts\/ci\/run-local-ci-equivalent\.sh/);

    const validatorConfig = cluster.config.agents.find((agent) => agent.id === 'validator');
    assert.deepStrictEqual(validatorConfig.requiredQualityGates, cluster.requiredQualityGates);
    assert.deepStrictEqual(validatorConfig.commandProofs, cluster.commandProofs);
  });
});
