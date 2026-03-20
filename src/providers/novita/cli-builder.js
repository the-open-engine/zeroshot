function buildCommand(context, options = {}) {
  const { modelSpec, outputFormat, jsonSchema, cwd, autoApprove, cliFeatures = {}, authEnv = {} } = options;

  const args = ['exec'];

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
  if (jsonSchema) {
    if (cliFeatures.supportsOutputSchema !== false) {
      const schemaStr =
        typeof jsonSchema === 'string' ? jsonSchema : JSON.stringify(jsonSchema);
      args.push('--output-schema', schemaStr);
    } else {
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
  }

  args.push(finalContext);

  return {
    binary: 'codex',
    args,
    env: authEnv,
  };
}

module.exports = {
  buildCommand,
};
