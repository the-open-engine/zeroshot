const assert = require('assert');

const { validateAgentConfig } = require('../src/agent/agent-config');
const { buildContext } = require('../src/agent/agent-context-builder');

function baseContextParams(config) {
  return {
    id: config.id,
    role: config.role,
    iteration: 1,
    config,
    messageBus: { query: () => [] },
    cluster: { id: 'test-cluster', createdAt: Date.now() - 60000 },
    triggeringMessage: { topic: 'IMPLEMENTATION_READY', sender: 'worker' },
  };
}

describe('required handoff quality gate context', function () {
  it('adds reusable command proof instructions for implementation agents', function () {
    const config = validateAgentConfig({
      id: 'worker',
      role: 'implementation',
      outputFormat: 'json',
      commandProofs: [
        {
          id: 'opcore-ci',
          profile: 'ci-equivalent',
          scope: 'repo',
          description: 'Opcore local CI',
          command: 'bash ./scripts/ci/run-local-ci-equivalent.sh',
        },
      ],
      prompt: 'Do the work.',
    });

    const context = buildContext(baseContextParams(config));

    assert.match(context, /Reusable Command Proofs/);
    assert.match(context, /id: opcore-ci, profile: ci-equivalent, scope: repo/);
    assert.match(context, /bash \.\/scripts\/ci\/run-local-ci-equivalent\.sh/);
    assert.match(context, /zeroshot cmdproof check opcore-ci/);
  });

  it('adds generic configured gate instructions and schema for validators', function () {
    const config = validateAgentConfig({
      id: 'validator',
      role: 'validator',
      outputFormat: 'json',
      jsonSchema: {
        type: 'object',
        properties: {
          approved: { type: 'boolean' },
        },
        required: ['approved'],
      },
      requiredQualityGates: [
        {
          id: 'repo-quality',
          scope: 'workspace',
          description: 'Run the configured workspace quality gate',
          command: 'quality-check --scope workspace',
          profile: 'ci-equivalent',
          commandProof: true,
        },
      ],
    });

    const context = buildContext(baseContextParams(config));

    assert.ok(config.jsonSchema.properties.qualityGates, 'schema should accept qualityGates');
    assert.deepStrictEqual(
      config.jsonSchema.properties.qualityGates.items.properties.evidence.required,
      ['command', 'exitCode', 'output']
    );
    assert.ok(
      config.jsonSchema.properties.qualityGates.items.properties.evidence.properties.proof,
      'schema should accept optional cmdproof evidence metadata'
    );
    assert.match(context, /Required Handoff Quality Gates/);
    assert.match(context, /id: repo-quality, scope: workspace/);
    assert.match(context, /Run the configured workspace quality gate/);
    assert.match(context, /quality-check --scope workspace/);
    assert.match(context, /zeroshot cmdproof check repo-quality/);
    assert.match(context, /publish one `qualityGates` entry/);
    assert.match(context, /approved` to false and publish status `FAIL`/);
    assert.match(context, /approved` to false and publish status `UNAVAILABLE`/);
  });

  it('does not add handoff gate instructions or schema when no gate is configured', function () {
    const config = validateAgentConfig({
      id: 'validator',
      role: 'validator',
      outputFormat: 'json',
      jsonSchema: {
        type: 'object',
        properties: {
          approved: { type: 'boolean' },
        },
        required: ['approved'],
      },
    });

    const context = buildContext(baseContextParams(config));

    assert.strictEqual(config.jsonSchema.properties.qualityGates, undefined);
    assert.ok(!context.includes('Required Handoff Quality Gates'));
  });
});
