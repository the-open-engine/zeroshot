/**
 * Regression test: structuredOutput → jsonSchema mapping
 *
 * ROOT CAUSE (discovered 2026-02-08):
 * git-pusher-template.js uses `structuredOutput` as the config key,
 * but agent-config.js only recognizes `jsonSchema`.
 * The structuredOutput key was silently ignored → default schema applied →
 * agent never told to output pr_number → verify_github_pr hook fails with
 * "VERIFICATION FAILED: git-pusher must provide pr_number in structured output"
 *
 * The PR was actually created and merged, but the hook couldn't extract
 * pr_number because the CLI was given the wrong schema.
 */

const assert = require('assert');
const { generateGitPusherAgent } = require('../src/agents/git-pusher-template');
const { validateAgentConfig } = require('../src/agent/agent-config');

describe('structuredOutput → jsonSchema mapping', function () {
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
});
