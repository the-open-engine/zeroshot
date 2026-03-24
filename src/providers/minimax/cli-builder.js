const path = require('path');

const CLI_WRAPPER_PATH = path.join(__dirname, 'cli-wrapper.js');

function buildCommand(context, options = {}) {
  const { modelSpec, outputFormat, jsonSchema } = options;

  let finalContext = context;
  if (jsonSchema) {
    const schemaStr =
      typeof jsonSchema === 'string' ? jsonSchema : JSON.stringify(jsonSchema, null, 2);
    finalContext =
      context +
      `\n\n## OUTPUT FORMAT (CRITICAL - REQUIRED)\n\nYou MUST respond with a JSON object that exactly matches this schema. NO markdown, NO explanation, NO code blocks. ONLY the raw JSON object.\n\nSchema:\n\`\`\`json\n${schemaStr}\n\`\`\`\n\nYour response must be ONLY valid JSON. Start with { and end with }. Nothing else.`;
  }

  const args = [CLI_WRAPPER_PATH];

  if (modelSpec?.model) {
    args.push('--model', modelSpec.model);
  }

  if (outputFormat === 'json' || outputFormat === 'stream-json') {
    args.push('--json');
  }

  args.push(finalContext);

  const env = {};
  if (process.env.MINIMAX_API_KEY) {
    env.MINIMAX_API_KEY = process.env.MINIMAX_API_KEY;
  }

  return {
    binary: process.execPath,
    args,
    env,
  };
}

module.exports = {
  buildCommand,
};
