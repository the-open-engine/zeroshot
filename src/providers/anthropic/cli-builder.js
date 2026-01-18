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

  addOutputFormatArgs(args, outputFormat, cliFeatures);
  addStreamJsonArgs(args, outputFormat, cliFeatures);
  addJsonSchemaArgs(args, outputFormat, jsonSchema, cliFeatures);
  addModelArgs(args, modelSpec, cliFeatures);
  addAutoApproveArgs(args, autoApprove, cliFeatures);

  args.push(context);

  return {
    binary: command,
    args,
    env: authEnv,
  };
}

function addOutputFormatArgs(args, outputFormat, cliFeatures) {
  if (!outputFormat || cliFeatures.supportsOutputFormat === false) {
    return;
  }
  args.push('--output-format', outputFormat);
}

function addStreamJsonArgs(args, outputFormat, cliFeatures) {
  if (outputFormat !== 'stream-json') {
    return;
  }
  if (cliFeatures.supportsVerbose !== false) {
    args.push('--verbose');
  }
  if (cliFeatures.supportsIncludePartials !== false) {
    args.push('--include-partial-messages');
  }
}

function addJsonSchemaArgs(args, outputFormat, jsonSchema, cliFeatures) {
  if (!jsonSchema || outputFormat !== 'json' || cliFeatures.supportsJsonSchema === false) {
    return;
  }
  const schemaString = typeof jsonSchema === 'string' ? jsonSchema : JSON.stringify(jsonSchema);
  args.push('--json-schema', schemaString);
}

function addModelArgs(args, modelSpec, cliFeatures) {
  if (!modelSpec?.model || cliFeatures.supportsModel === false) {
    return;
  }
  args.push('--model', modelSpec.model);
}

function addAutoApproveArgs(args, autoApprove, cliFeatures) {
  if (!autoApprove || cliFeatures.supportsAutoApprove === false) {
    return;
  }
  args.push('--dangerously-skip-permissions');
}

module.exports = {
  buildCommand,
};
