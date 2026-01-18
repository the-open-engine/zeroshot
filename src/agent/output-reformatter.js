/**
 * Output Reformatter - Convert non-JSON output to valid JSON
 *
 * When an LLM outputs markdown/text instead of JSON despite schema instructions,
 * this module attempts to extract/reformat the content into valid JSON.
 *
 * STATUS: SDK NOT IMPLEMENTED - Reformatting is not available.
 * This module exists for future extension when SDK support is added.
 *
 * To enable reformatting:
 * 1. Implement SDK support in the provider (getSDKEnvVar, callSimple)
 * 2. The reformatOutput() function will then work automatically
 */

const DEFAULT_MAX_ATTEMPTS = 3;

/**
 * Build the reformatting prompt
 *
 * @param {string} rawOutput - The non-JSON output to reformat
 * @param {Object} schema - Target JSON schema
 * @param {string|null} previousError - Error from previous attempt (for feedback)
 * @returns {string} The prompt for the reformatting model
 */
function buildReformatPrompt(rawOutput, schema, previousError = null) {
  const schemaStr = JSON.stringify(schema, null, 2);
  // Truncate long outputs to avoid context limits
  const truncatedOutput = rawOutput.length > 4000 ? rawOutput.slice(-4000) : rawOutput;

  let prompt = `Convert this text into a JSON object matching the schema.

## SCHEMA
\`\`\`json
${schemaStr}
\`\`\`

## TEXT TO CONVERT
\`\`\`
${truncatedOutput}
\`\`\`

## RULES
- Output ONLY the JSON object
- NO markdown code blocks
- NO explanations
- Start with { end with }
- Match ALL required fields from schema`;

  if (previousError) {
    prompt += `

## PREVIOUS ATTEMPT FAILED
Error: ${previousError}
Fix this issue in your response.`;
  }

  return prompt;
}

/**
 * Attempt to reformat non-JSON output into valid JSON
 *
 * STATUS: SDK NOT IMPLEMENTED - This function always throws.
 * When SDK support is added to providers, this will work automatically.
 *
 * @param {Object} options
 * @param {string} options.rawOutput - The non-JSON output to reformat
 * @param {Object} options.schema - Target JSON schema
 * @param {string} options.providerName - Provider name (claude, codex, gemini, opencode)
 * @param {number} [options.maxAttempts=3] - Maximum reformatting attempts
 * @param {Function} [options.onAttempt] - Callback for each attempt (attempt, error)
 * @returns {Promise<Object>} The reformatted JSON object
 * @throws {Error} Always throws - SDK not implemented
 */
function reformatOutput({
  rawOutput,
  schema: _schema,
  providerName,
  maxAttempts: _maxAttempts = DEFAULT_MAX_ATTEMPTS,
  onAttempt: _onAttempt,
}) {
  // SDK not implemented - reformatting not available
  // When SDK support is added, uncomment the implementation below
  return Promise.reject(
    new Error(
      `Output reformatting not available: SDK not implemented for provider "${providerName}". ` +
        `Agent output must be valid JSON. Raw output (last 200 chars): ${(rawOutput || '').slice(-200)}`
    )
  );

  // FUTURE: When SDK support is added to providers, uncomment this:
  /*
  const { getProvider } = require('../providers');
  const provider = getProvider(providerName);

  let lastError = null;

  for (let attempt = 1; attempt <= maxAttempts; attempt++) {
    if (onAttempt) {
      onAttempt(attempt, lastError);
    }

    const prompt = buildReformatPrompt(rawOutput, schema, lastError);

    try {
      const result = await provider.callSimple(prompt, {
        level: 'level1',
        maxTokens: 2000,
      });

      if (!result?.success) {
        lastError = result?.error || 'API call failed';
        continue;
      }

      if (!result?.text) {
        lastError = 'Empty response from reformatting model';
        continue;
      }

      const parsed = extractJsonFromOutput(result.text, providerName);

      if (!parsed) {
        lastError = 'Could not extract JSON from reformatted output';
        continue;
      }

      const validationError = validateAgainstSchema(parsed, schema);
      if (validationError) {
        lastError = validationError;
        continue;
      }

      return parsed;
    } catch (err) {
      lastError = err.message;
    }
  }

  throw new Error(
    `Failed to reformat output after ${maxAttempts} attempts. Last error: ${lastError}`
  );
  */
}

/**
 * Validate parsed output against JSON schema
 *
 * @param {Object} parsed - Parsed JSON object
 * @param {Object} schema - JSON schema to validate against
 * @returns {string|null} Error message if validation failed, null if valid
 */
function validateAgainstSchema(parsed, schema) {
  const Ajv = require('ajv');
  const ajv = new Ajv({ allErrors: true, strict: false });
  const validate = ajv.compile(schema);
  const valid = validate(parsed);

  if (!valid) {
    const errors = (validate.errors || [])
      .slice(0, 3)
      .map((e) => `${e.instancePath || '#'} ${e.message}`)
      .join('; ');
    return errors || 'Schema validation failed';
  }

  return null;
}

module.exports = {
  reformatOutput,
  buildReformatPrompt,
  validateAgainstSchema,
  DEFAULT_MAX_ATTEMPTS,
};
