function buildCommand(context, options = {}) {
  const { modelSpec, jsonSchema, autoApprove, mcpConfig, addDirs, cliFeatures = {} } = options;

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

  // MCP servers — Copilot CLI augments ~/.copilot/mcp-config.json with --additional-mcp-config.
  // Accepts either a JSON string, a config object (will be JSON.stringified),
  // a file path prefixed with @, or an array of any of the above (each emits one flag).
  if (mcpConfig && cliFeatures.supportsMcpConfig !== false) {
    const entries = Array.isArray(mcpConfig) ? mcpConfig : [mcpConfig];
    for (const entry of entries) {
      if (entry === null || entry === undefined) continue;
      const value = typeof entry === 'string' ? entry : JSON.stringify(entry);
      args.push('--additional-mcp-config', value);
    }
  }

  // Allow extra directories for file access (useful when running outside cwd)
  if (Array.isArray(addDirs) && cliFeatures.supportsAddDir !== false) {
    for (const dir of addDirs) {
      if (typeof dir === 'string' && dir) args.push('--add-dir', dir);
    }
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
