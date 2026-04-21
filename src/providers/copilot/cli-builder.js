function buildCommand(context, options = {}) {
  const { modelSpec, jsonSchema, autoApprove, cliFeatures = {} } = options;

  let finalContext = context;
  if (jsonSchema) {
    const schemaStr =
      typeof jsonSchema === 'string' ? jsonSchema : JSON.stringify(jsonSchema, null, 2);
    finalContext =
      context +
      `\n\n## OUTPUT FORMAT (CRITICAL - REQUIRED)\n\nYou MUST respond with a JSON object that exactly matches this schema. NO markdown, NO explanation, NO code blocks. ONLY the raw JSON object.\n\nSchema:\n\`\`\`json\n${schemaStr}\n\`\`\`\n\nYour response must be ONLY valid JSON. Start with { and end with }. Nothing else.`;
  }

  const args = ['-p', finalContext];

  if (cliFeatures.supportsSilent !== false) {
    args.push('--silent');
  }

  if (autoApprove !== false && cliFeatures.supportsAllowAll !== false) {
    args.push('--allow-all');
  }

  if (modelSpec?.model && cliFeatures.supportsModel !== false) {
    args.push('--model', modelSpec.model);
  }

  return {
    binary: 'copilot',
    args,
    env: {},
  };
}

module.exports = {
  buildCommand,
};
