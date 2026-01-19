function buildCommand(context, options = {}) {
  const { modelSpec, outputFormat, jsonSchema, cwd, autoApprove, cliFeatures = {} } = options;

  // Augment context with schema if provided (Gemini CLI doesn't support native schema enforcement)
  let finalContext = context;
  if (jsonSchema) {
    // CRITICAL: Inject schema into prompt since Gemini CLI has no --output-schema flag
    // Without this, model outputs free-form text instead of JSON
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

  const args = ['-p', finalContext];

  if (
    (outputFormat === 'stream-json' || outputFormat === 'json') &&
    cliFeatures.supportsStreamJson
  ) {
    args.push('--output-format', 'stream-json');
  }

  if (modelSpec?.model) {
    args.push('-m', modelSpec.model);
  }

  if (cwd && cliFeatures.supportsCwd) {
    args.push('--cwd', cwd);
  }

  if (autoApprove && cliFeatures.supportsAutoApprove) {
    args.push('--yolo');
  }

  return {
    binary: 'gemini',
    args,
    env: {},
  };
}

module.exports = {
  buildCommand,
};
