function buildCommand(context, options = {}) {
  const { modelSpec, outputFormat, jsonSchema, cwd, cliFeatures = {} } = options;

  let finalContext = context;
  if (jsonSchema) {
    const schemaStr =
      typeof jsonSchema === 'string' ? jsonSchema : JSON.stringify(jsonSchema, null, 2);
    finalContext =
      context +
      `\n\n## OUTPUT FORMAT (CRITICAL - REQUIRED)\n\nYou MUST respond with a JSON object that exactly matches this schema. NO markdown, NO explanation, NO code blocks. ONLY the raw JSON object.\n\nSchema:\n\`\`\`json\n${schemaStr}\n\`\`\`\n\nYour response must be ONLY valid JSON. Start with { and end with }. Nothing else.`;
  }

  const args = ['run'];

  if ((outputFormat === 'stream-json' || outputFormat === 'json') && cliFeatures.supportsJson) {
    args.push('--format', 'json');
  }

  if (modelSpec?.model) {
    args.push('--model', modelSpec.model);
  }

  if (modelSpec?.reasoningEffort && cliFeatures.supportsVariant) {
    args.push('--variant', modelSpec.reasoningEffort);
  }

  if (cwd && cliFeatures.supportsCwd) {
    args.push('--cwd', cwd);
  }

  args.push(finalContext);

  return {
    binary: 'opencode',
    args,
    env: {},
  };
}

module.exports = {
  buildCommand,
};
