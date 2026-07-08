/**
 * Regression test: structuredOutput → jsonSchema mapping
 *
 * ROOT CAUSE (discovered 2026-02-08):
 * git-pusher-template.js uses `structuredOutput` as the config key,
 * but agent-config.js only recognizes `jsonSchema`.
 * The structuredOutput key was silently ignored → default schema applied →
 * agent never told to output pr_number → verify_pull_request hook fails with
 * "VERIFICATION FAILED: git-pusher must provide pr_number in structured output"
 *
 * The PR was actually created and merged, but the hook couldn't extract
 * pr_number because the CLI was given the wrong schema.
 */

const assert = require('assert');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { execFileSync } = require('child_process');
const { generateGitPusherAgent } = require('../src/agents/git-pusher-template');
const { validateAgentConfig } = require('../src/agent/agent-config');
const {
  resolveClusterRequiredQualityGates,
  resolveRequiredQualityGates,
} = require('../src/quality-gates');

describe('structuredOutput → jsonSchema mapping', function () {
  function createRepoWithSettings(settings) {
    const repoDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-settings-gates-'));
    execFileSync('git', ['init'], { cwd: repoDir, stdio: 'ignore' });
    const settingsDir = path.join(repoDir, '.zeroshot');
    fs.mkdirSync(settingsDir, { recursive: true });
    fs.writeFileSync(path.join(settingsDir, 'settings.json'), JSON.stringify(settings, null, 2));
    return repoDir;
  }

  it('should use structuredOutput as jsonSchema when both are not set', function () {
    // SETUP: Generate git-pusher config (uses structuredOutput key)
    const agentConfig = generateGitPusherAgent('github');

    // VERIFY: structuredOutput is defined in the raw config
    assert.ok(agentConfig.structuredOutput, 'git-pusher template must define structuredOutput');
    assert.ok(
      agentConfig.structuredOutput.properties.pr_number,
      'structuredOutput must have pr_number property'
    );

    // ACTION: Pass through validateAgentConfig (this is where mapping should happen)
    const normalized = validateAgentConfig({ ...agentConfig });

    // ASSERTION: jsonSchema must be the structuredOutput schema, NOT the default
    assert.ok(normalized.jsonSchema, 'jsonSchema must be set after validation');
    assert.ok(
      normalized.jsonSchema.properties.pr_number,
      'jsonSchema must contain pr_number from structuredOutput (not default summary/result schema)'
    );
    assert.strictEqual(
      normalized.jsonSchema.properties.pr_number.type,
      'number',
      'pr_number must be type number'
    );

    // Verify it does NOT have the default schema fields
    assert.strictEqual(
      normalized.jsonSchema.properties.summary,
      undefined,
      'jsonSchema must NOT have default "summary" field when structuredOutput is provided'
    );
  });

  it('should preserve explicit jsonSchema over structuredOutput', function () {
    // If someone sets BOTH jsonSchema and structuredOutput, jsonSchema wins
    const agentConfig = generateGitPusherAgent('github');
    const customSchema = {
      type: 'object',
      properties: {
        custom_field: { type: 'string' },
      },
      required: ['custom_field'],
    };

    const normalized = validateAgentConfig({
      ...agentConfig,
      jsonSchema: customSchema,
    });

    assert.strictEqual(
      normalized.jsonSchema.properties.custom_field.type,
      'string',
      'explicit jsonSchema must take precedence over structuredOutput'
    );
  });

  it('should apply default schema when neither jsonSchema nor structuredOutput is set', function () {
    const normalized = validateAgentConfig({
      id: 'test-agent',
      role: 'test',
      triggers: [],
      prompt: 'test prompt',
    });

    assert.ok(
      normalized.jsonSchema.properties.summary,
      'default schema must have summary when no schema provided'
    );
    assert.ok(
      normalized.jsonSchema.properties.result,
      'default schema must have result when no schema provided'
    );
  });

  it('should work for all git-pusher platforms', function () {
    const platforms = ['github', 'gitlab', 'azure-devops'];

    for (const platform of platforms) {
      const agentConfig = generateGitPusherAgent(platform);
      const normalized = validateAgentConfig({ ...agentConfig });

      assert.ok(
        normalized.jsonSchema.properties.pr_number || normalized.jsonSchema.properties.mr_number,
        `${platform}: jsonSchema must contain pr_number or mr_number from structuredOutput`
      );
    }
  });

  it('keeps git-pusher transport-only after validator handoff', function () {
    const agentConfig = generateGitPusherAgent('github');
    const prompt = agentConfig.prompt;

    assert.ok(
      prompt.includes('TRANSPORT-ONLY GIT PUSHER'),
      'git-pusher prompt must describe the transport-only role'
    );
    assert.ok(
      prompt.includes('Do NOT edit source files'),
      'git-pusher prompt must forbid post-validation source edits'
    );
    assert.ok(
      prompt.includes('Do NOT inspect CI logs to debug product code'),
      'git-pusher prompt must forbid CI debugging after validator handoff'
    );
    assert.ok(
      prompt.includes('blocked_reason'),
      'git-pusher prompt must provide blocked JSON for handoff failures'
    );
    assert.ok(
      agentConfig.structuredOutput.properties.blocked,
      'git-pusher schema must include blocked flag'
    );
    assert.ok(
      agentConfig.structuredOutput.properties.blocked_reason,
      'git-pusher schema must include blocked reason'
    );
    assert.ok(
      !prompt.includes('debug and fix it'),
      'git-pusher prompt must not tell the agent to debug and fix failures'
    );
    assert.ok(
      !prompt.includes('RESOLVE THEM IMMEDIATELY'),
      'git-pusher prompt must not tell the agent to resolve merge conflicts'
    );
  });

  it('should include required quality gates from explicit generator options', function () {
    const agentConfig = generateGitPusherAgent('github', {
      requiredQualityGates: [
        {
          id: 'repo-quality',
          scope: 'workspace',
          description: 'Required repository quality gate',
          command: 'quality-check --scope workspace',
        },
      ],
    });

    assert.deepStrictEqual(agentConfig.requiredQualityGates, [
      {
        id: 'repo-quality',
        scope: 'workspace',
        description: 'Required repository quality gate',
        command: 'quality-check --scope workspace',
      },
    ]);
  });

  it('should resolve required quality gates from repo settings', function () {
    const gate = {
      id: 'repo-quality',
      scope: 'settings',
      description: 'Settings-defined quality gate',
      command: 'quality-check --scope settings',
    };
    const repoDir = createRepoWithSettings({ ship: { requiredQualityGates: [gate] } });
    try {
      assert.deepStrictEqual(resolveRequiredQualityGates({ cwd: repoDir }), [gate]);
      assert.deepStrictEqual(
        generateGitPusherAgent('github', { cwd: repoDir }).requiredQualityGates,
        [gate]
      );
    } finally {
      fs.rmSync(repoDir, { recursive: true, force: true });
    }
  });

  it('should not let undefined CLI quality gates mask repo settings', function () {
    const gate = {
      id: 'repo-quality',
      scope: 'settings',
      description: 'Settings-defined quality gate',
      command: 'quality-check --scope settings',
    };
    const repoDir = createRepoWithSettings({ ship: { requiredQualityGates: [gate] } });
    try {
      const options = { cwd: repoDir, requiredQualityGates: undefined };

      assert.deepStrictEqual(resolveRequiredQualityGates(options), [gate]);
      assert.deepStrictEqual(resolveClusterRequiredQualityGates({}, options), [gate]);
      assert.deepStrictEqual(generateGitPusherAgent('github', options).requiredQualityGates, [
        gate,
      ]);
    } finally {
      fs.rmSync(repoDir, { recursive: true, force: true });
    }
  });

  it('should not let undefined CLI quality gates mask non-ship settings', function () {
    const repoGate = {
      id: 'repo-quality',
      scope: 'settings',
      description: 'Settings-defined quality gate',
      command: 'quality-check --scope settings',
    };
    const clusterGate = {
      id: 'cluster-quality',
      scope: 'config',
      description: 'Cluster-defined quality gate',
      command: 'quality-check --scope config',
    };
    const repoDir = createRepoWithSettings({ requiredQualityGates: [repoGate] });
    try {
      const options = { cwd: repoDir, requiredQualityGates: undefined };

      assert.deepStrictEqual(resolveRequiredQualityGates(options), [repoGate]);
      assert.deepStrictEqual(generateGitPusherAgent('github', options).requiredQualityGates, [
        repoGate,
      ]);
      assert.deepStrictEqual(
        resolveClusterRequiredQualityGates({ requiredQualityGates: [clusterGate] }, options),
        [clusterGate]
      );
    } finally {
      fs.rmSync(repoDir, { recursive: true, force: true });
    }
  });
});
