function buildCommand(context, options = {}, commandConfig = {}) {
  const {
    modelSpec,
    outputFormat,
    jsonSchema,
    autoApprove,
    cliFeatures = {},
    authEnv = {},
  } = options;

  const command = commandConfig.command || 'claude';
  const extraArgs = commandConfig.args || [];
  const args = [...extraArgs, '--print', '--input-format', 'text'];

  if (outputFormat && cliFeatures.supportsOutputFormat !== false) {
    args.push('--output-format', outputFormat);
  }

  if (outputFormat === 'stream-json') {
    if (cliFeatures.supportsVerbose !== false) {
      args.push('--verbose');
    }
    if (cliFeatures.supportsIncludePartials !== false) {
      args.push('--include-partial-messages');
    }
  }

  if (jsonSchema && outputFormat === 'json' && cliFeatures.supportsJsonSchema !== false) {
    const schemaString = typeof jsonSchema === 'string' ? jsonSchema : JSON.stringify(jsonSchema);
    args.push('--json-schema', schemaString);
  }

  if (modelSpec?.model && cliFeatures.supportsModel !== false) {
    args.push('--model', modelSpec.model);
  }

  if (autoApprove && cliFeatures.supportsAutoApprove !== false) {
    args.push('--dangerously-skip-permissions');
  }

  args.push(context);

  return {
    binary: command,
    args,
    env: authEnv,
  };
}

module.exports = {
  buildCommand,
};
