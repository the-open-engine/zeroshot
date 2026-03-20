const fs = require('fs');
const path = require('path');
const os = require('os');

/**
 * Codex --output-schema requires strict schema constraints:
 * 1. additionalProperties: false on ALL object schemas
 * 2. 'required' must include ALL keys in 'properties'
 * This function recursively enforces both constraints.
 * @param {Object} schema - JSON Schema object
 * @returns {Object} - Modified schema compliant with Codex structured output requirements
 */
function enforceStrictSchema(schema) {
  if (!schema || typeof schema !== 'object') return schema;

  const result = { ...schema };

  if (result.type === 'object') {
    result.additionalProperties = false;
    if (result.properties) {
      result.required = Object.keys(result.properties);
    }
  }

  if (result.properties) {
    result.properties = {};
    for (const [key, value] of Object.entries(schema.properties)) {
      result.properties[key] = enforceStrictSchema(value);
    }
  }

  if (result.items) {
    result.items = enforceStrictSchema(schema.items);
  }

  for (const key of ['anyOf', 'oneOf', 'allOf']) {
    if (Array.isArray(result[key])) {
      result[key] = result[key].map(enforceStrictSchema);
    }
  }

  if (result.additionalProperties && typeof result.additionalProperties === 'object') {
    result.additionalProperties = enforceStrictSchema(result.additionalProperties);
  }

  return result;
}

function buildCommand(context, options = {}) {
  const { modelSpec, outputFormat, jsonSchema, cwd, autoApprove, cliFeatures = {}, authEnv = {} } = options;

  const args = ['exec'];
  const cleanup = [];

  if ((outputFormat === 'stream-json' || outputFormat === 'json') && cliFeatures.supportsJson) {
    args.push('--json');
  }

  if (modelSpec?.model) {
    args.push('-m', modelSpec.model);
  }

  if (modelSpec?.reasoningEffort && cliFeatures.supportsConfigOverride) {
    args.push('--config', `model_reasoning_effort="${modelSpec.reasoningEffort}"`);
  }

  if (cwd && cliFeatures.supportsCwd) {
    args.push('-C', cwd);
  }

  if (autoApprove && cliFeatures.supportsAutoApprove) {
    args.push('--dangerously-bypass-approvals-and-sandbox');
  }

  // Always skip git repo check - zeroshot runs non-interactively and may run in any directory
  if (cliFeatures.supportsSkipGitRepoCheck !== false) {
    args.push('--skip-git-repo-check');
  }

  let finalContext = context;
  if (jsonSchema && cliFeatures.supportsOutputSchema) {
    // CRITICAL: Codex --output-schema takes a FILE PATH, not a JSON string
    // Codex requires additionalProperties: false on all object schemas
    const parsedSchema = typeof jsonSchema === 'string' ? JSON.parse(jsonSchema) : jsonSchema;
    const strictSchema = enforceStrictSchema(parsedSchema);
    const schemaStr = JSON.stringify(strictSchema, null, 2);
    const schemaFile = path.join(
      os.tmpdir(),
      `zeroshot-schema-${Date.now()}-${Math.random().toString(36).slice(2)}.json`
    );
    fs.writeFileSync(schemaFile, schemaStr);
    cleanup.push(schemaFile);
    args.push('--output-schema', schemaFile);
  } else if (jsonSchema) {
    // Fall back to prompt injection when --output-schema is not available
    const schemaStr =
      typeof jsonSchema === 'string' ? jsonSchema : JSON.stringify(jsonSchema, null, 2);
    finalContext =
      context +
      `\n\n## OUTPUT FORMAT (CRITICAL - REQUIRED)

You MUST respond with a JSON object that exactly matches this schema. NO markdown, NO explanation, NO code blocks. ONLY the raw JSON object.

Schema:
\`\`\`json
${schemaStr}
\`\`\`

Your response must be ONLY valid JSON. Start with { and end with }. Nothing else.`;
  }

  args.push(finalContext);

  return {
    binary: 'codex',
    args,
    env: authEnv,
    cleanup,
  };
}

module.exports = {
  buildCommand,
};
