/**
 * AgentContextBuilder - Build agent execution context from ledger
 *
 * Provides:
 * - Context assembly from multiple message sources
 * - Context strategy evaluation (topics, limits, since timestamps)
 * - Prompt injection and formatting
 * - Token-based truncation
 * - Defensive context overflow prevention
 */

// Defensive limit: 500,000 chars ≈ 125k tokens (safe buffer below 200k limit)
// Prevents "Prompt is too long" errors that kill tasks
const MAX_CONTEXT_CHARS = 500000;

/**
 * Generate an example object from a JSON schema
 * Used to show models a concrete example of expected output
 *
 * @param {object} schema - JSON schema
 * @returns {object|null} Example object or null if generation fails
 */
function generateExampleFromSchema(schema) {
  if (!schema || schema.type !== 'object' || !schema.properties) {
    return null;
  }

  const example = {};

  for (const [key, propSchema] of Object.entries(schema.properties)) {
    if (propSchema.enum && propSchema.enum.length > 0) {
      // Use first enum value as example
      example[key] = propSchema.enum[0];
    } else if (propSchema.type === 'string') {
      example[key] = propSchema.description || `${key} value`;
    } else if (propSchema.type === 'boolean') {
      example[key] = true;
    } else if (propSchema.type === 'number' || propSchema.type === 'integer') {
      example[key] = 0;
    } else if (propSchema.type === 'array') {
      if (propSchema.items?.type === 'string') {
        example[key] = [];
      } else {
        example[key] = [];
      }
    } else if (propSchema.type === 'object') {
      example[key] = generateExampleFromSchema(propSchema) || {};
    }
  }

  return example;
}

function buildAutonomousSection() {
  let context = `## 🔴 CRITICAL: AUTONOMOUS EXECUTION REQUIRED\n\n`;
  context += `You are running in a NON-INTERACTIVE cluster environment.\n\n`;
  context += `**NEVER** use AskUserQuestion or ask for user input - there is NO user to respond.\n`;
  context += `**NEVER** ask "Would you like me to..." or "Should I..." - JUST DO IT.\n`;
  context += `**NEVER** wait for approval or confirmation - MAKE DECISIONS AUTONOMOUSLY.\n\n`;
  context += `When facing choices:\n`;
  context += `- Choose the option that maintains code quality and correctness\n`;
  context += `- If unsure between "fix the code" vs "relax the rules" → ALWAYS fix the code\n`;
  context += `- If unsure between "do more" vs "do less" → ALWAYS do what's required, nothing more\n\n`;
  return context;
}

function buildOutputStyleSection() {
  let context = `## 🔴 OUTPUT STYLE - NON-NEGOTIABLE\n\n`;
  context += `**ALL OUTPUT: Maximum informativeness, minimum verbosity. NO EXCEPTIONS.**\n\n`;
  context += `This applies to EVERYTHING you output:\n`;
  context += `- Text responses\n`;
  context += `- JSON schema values\n`;
  context += `- Reasoning fields\n`;
  context += `- Summary fields\n`;
  context += `- ALL string values in structured output\n\n`;
  context += `Rules:\n`;
  context += `- Progress: "Reading auth.ts" NOT "I will now read the auth.ts file..."\n`;
  context += `- Tool calls: NO preamble. Call immediately.\n`;
  context += `- Schema strings: Dense facts. No filler. No fluff.\n`;
  context += `- Errors: DETAILED (stack traces, repro). NEVER compress errors.\n`;
  context += `- FORBIDDEN: "I'll help...", "Let me...", "I'm going to...", "Sure!", "Great!", "Certainly!"\n\n`;
  context += `Every token costs money. Waste nothing.\n\n`;
  return context;
}

function buildGitOperationsSection() {
  let context = `## 🚫 GIT OPERATIONS - FORBIDDEN\n\n`;
  context += `NEVER commit, push, or create PRs. You only modify files.\n`;
  context += `The git-pusher agent handles ALL git operations AFTER validators approve.\n\n`;
  context += `- ❌ NEVER run: git add, git commit, git push, gh pr create\n`;
  context += `- ❌ NEVER suggest committing changes\n`;
  context += `- ✅ Only modify files and publish your completion message when done\n\n`;
  return context;
}

function buildHeaderContext({ id, role, iteration, isIsolated }) {
  let context = `You are agent "${id}" with role "${role}".\n\n`;
  context += `Iteration: ${iteration}\n\n`;
  context += buildAutonomousSection();
  context += buildOutputStyleSection();
  if (!isIsolated) {
    context += buildGitOperationsSection();
  }
  return context;
}

function buildInstructionsSection({ config, selectedPrompt, id }) {
  const promptText =
    selectedPrompt || (typeof config.prompt === 'string' ? config.prompt : config.prompt?.system);

  if (promptText) {
    return `## Instructions\n\n${promptText}\n\n`;
  }

  if (config.prompt && typeof config.prompt !== 'string' && !config.prompt?.system) {
    throw new Error(
      `Agent "${id}" has invalid prompt format. ` +
        `Expected string or object with .system property, got: ${JSON.stringify(config.prompt).slice(0, 100)}...`
    );
  }

  return '';
}

function buildLegacyOutputSchemaSection(config) {
  if (!config.prompt?.outputFormat) return '';

  let context = `## Output Schema (REQUIRED)\n\n`;
  context += `\`\`\`json\n${JSON.stringify(config.prompt.outputFormat.example, null, 2)}\n\`\`\`\n\n`;
  context += `STRING VALUES IN THIS SCHEMA: Dense. Factual. No filler words. No pleasantries.\n`;
  if (config.prompt.outputFormat.rules) {
    for (const rule of config.prompt.outputFormat.rules) {
      context += `- ${rule}\n`;
    }
  }
  context += '\n';
  return context;
}

function buildJsonSchemaSection(config) {
  if (!config.jsonSchema || config.outputFormat !== 'json') return '';

  let context = `## 🔴 OUTPUT FORMAT - JSON ONLY\n\n`;
  context += `Your response must be ONLY valid JSON. No other text before or after.\n`;
  context += `Start with { and end with }. Nothing else.\n\n`;
  context += `Required schema:\n`;
  context += `\`\`\`json\n${JSON.stringify(config.jsonSchema, null, 2)}\n\`\`\`\n\n`;

  const example = generateExampleFromSchema(config.jsonSchema);
  if (example) {
    context += `Example output:\n`;
    context += `\`\`\`json\n${JSON.stringify(example, null, 2)}\n\`\`\`\n\n`;
  }

  context += `CRITICAL RULES:\n`;
  context += `- Output ONLY the JSON object - no explanation, no thinking, no preamble\n`;
  context += `- Use EXACTLY the enum values specified (case-sensitive)\n`;
  context += `- Include ALL required fields\n\n`;
  return context;
}

function resolveSourceSince(source, cluster, lastTaskEndTime, lastAgentStartTime) {
  const sinceValue = source.since;

  if (sinceValue === 'cluster_start') {
    return cluster.createdAt;
  }
  if (sinceValue === 'last_task_end') {
    return lastTaskEndTime || cluster.createdAt;
  }
  if (sinceValue === 'last_agent_start') {
    return lastAgentStartTime || cluster.createdAt;
  }

  if (typeof sinceValue === 'string') {
    const parsed = Date.parse(sinceValue);
    if (Number.isNaN(parsed)) {
      throw new Error(
        `Agent context source for topic ${source.topic} has invalid since value "${sinceValue}". ` +
          'Use cluster_start, last_task_end, last_agent_start, or an ISO timestamp.'
      );
    }
    return parsed;
  }

  return sinceValue;
}

function formatSourceMessagesSection(source, messages) {
  let context = `\n## Messages from topic: ${source.topic}\n\n`;
  for (const msg of messages) {
    context += `[${new Date(msg.timestamp).toISOString()}] ${msg.sender}:\n`;
    if (msg.content?.text) {
      context += `${msg.content.text}\n`;
    }
    if (msg.content?.data) {
      context += `Data: ${JSON.stringify(msg.content.data, null, 2)}\n`;
    }
    context += '\n';
  }
  return context;
}

function buildSourcesSection({
  strategy,
  messageBus,
  cluster,
  lastTaskEndTime,
  lastAgentStartTime,
}) {
  let context = '';
  for (const source of strategy.sources) {
    const sinceTimestamp = resolveSourceSince(source, cluster, lastTaskEndTime, lastAgentStartTime);
    const messages = messageBus.query({
      cluster_id: cluster.id,
      topic: source.topic,
      sender: source.sender,
      since: sinceTimestamp,
      limit: source.limit,
    });

    if (messages.length > 0) {
      context += formatSourceMessagesSection(source, messages);
    }
  }
  return context;
}

function collectCannotValidateCriteria(prevValidations) {
  const cannotValidateCriteria = [];
  for (const msg of prevValidations) {
    const criteriaResults = msg.content?.data?.criteriaResults;
    if (!Array.isArray(criteriaResults)) continue;
    for (const cr of criteriaResults) {
      if (cr.status !== 'CANNOT_VALIDATE' || !cr.id) continue;
      if (cannotValidateCriteria.find((c) => c.id === cr.id)) continue;
      cannotValidateCriteria.push({
        id: cr.id,
        reason: cr.reason || 'No reason provided',
      });
    }
  }
  return cannotValidateCriteria;
}

function buildCannotValidateSection(cannotValidateCriteria) {
  if (cannotValidateCriteria.length === 0) return '';

  let context = `\n## ⚠️ Permanently Unverifiable Criteria (SKIP THESE)\n\n`;
  context += `The following criteria have PERMANENT environmental limitations (missing tools, no access).\n`;
  context += `These limitations have not changed. Do NOT re-attempt verification.\n`;
  context += `Mark these as CANNOT_VALIDATE again with the same reason.\n\n`;
  for (const cv of cannotValidateCriteria) {
    context += `- **${cv.id}**: ${cv.reason}\n`;
  }
  context += `\n`;
  return context;
}

function buildValidatorSkipSection({ role, messageBus, cluster }) {
  if (role !== 'validator') return '';

  const prevValidations = messageBus.query({
    cluster_id: cluster.id,
    topic: 'VALIDATION_RESULT',
    since: cluster.createdAt,
    limit: 50,
  });

  const cannotValidateCriteria = collectCannotValidateCriteria(prevValidations);
  return buildCannotValidateSection(cannotValidateCriteria);
}

function buildTriggeringMessageSection(triggeringMessage) {
  let context = `\n## Triggering Message\n\n`;
  context += `Topic: ${triggeringMessage.topic}\n`;
  context += `Sender: ${triggeringMessage.sender}\n`;
  if (triggeringMessage.content?.text) {
    context += `\n${triggeringMessage.content.text}\n`;
  }
  return context;
}

function findContextSectionIndices(lines) {
  let issueOpenedStart = -1;
  let issueOpenedEnd = -1;
  let triggeringStart = -1;

  for (let i = 0; i < lines.length; i++) {
    if (lines[i].includes('## Messages from topic: ISSUE_OPENED')) {
      issueOpenedStart = i;
    }
    if (issueOpenedStart !== -1 && issueOpenedEnd === -1 && lines[i].startsWith('## ')) {
      issueOpenedEnd = i;
    }
    if (lines[i].includes('## Triggering Message')) {
      triggeringStart = i;
      break;
    }
  }

  return { issueOpenedStart, issueOpenedEnd, triggeringStart };
}

function collectRecentLines(middleLines, budgetForRecent) {
  const recentLines = [];
  let recentSize = 0;

  for (let i = middleLines.length - 1; i >= 0; i--) {
    const line = middleLines[i];
    const lineSize = line.length + 1;

    if (recentSize + lineSize > budgetForRecent) {
      break;
    }

    recentLines.unshift(line);
    recentSize += lineSize;
  }

  return recentLines;
}

function truncateContextIfNeeded(context) {
  const originalLength = context.length;
  if (originalLength <= MAX_CONTEXT_CHARS) {
    return context;
  }

  console.log(
    `[Context] Context too large (${originalLength} chars), truncating to prevent overflow...`
  );

  const lines = context.split('\n');
  const { issueOpenedStart, issueOpenedEnd, triggeringStart } = findContextSectionIndices(lines);

  const headerEnd = issueOpenedStart !== -1 ? issueOpenedStart : triggeringStart;
  const header = lines.slice(0, headerEnd).join('\n');

  const issueOpened =
    issueOpenedStart !== -1 && issueOpenedEnd !== -1
      ? lines.slice(issueOpenedStart, issueOpenedEnd).join('\n')
      : '';

  const triggeringMsg = lines.slice(triggeringStart).join('\n');

  const fixedSize = header.length + issueOpened.length + triggeringMsg.length;
  const budgetForRecent = MAX_CONTEXT_CHARS - fixedSize - 200;

  const middleStart = issueOpenedEnd !== -1 ? issueOpenedEnd : headerEnd;
  const middleEnd = triggeringStart;
  const middleLines = lines.slice(middleStart, middleEnd);
  const recentLines = collectRecentLines(middleLines, budgetForRecent);

  const parts = [header];
  if (issueOpened) {
    parts.push(issueOpened);
  }
  if (recentLines.length < middleLines.length) {
    const truncatedCount = middleLines.length - recentLines.length;
    parts.push(
      `\n[...${truncatedCount} earlier context messages truncated to prevent overflow...]\n`
    );
  }
  if (recentLines.length > 0) {
    parts.push(recentLines.join('\n'));
  }
  parts.push(triggeringMsg);

  const truncatedContext = parts.join('\n');
  const truncatedLength = truncatedContext.length;
  console.log(
    `[Context] Truncated from ${originalLength} to ${truncatedLength} chars (${Math.round((truncatedLength / originalLength) * 100)}% retained)`
  );

  return truncatedContext;
}

function applyLegacyMaxTokens(context, strategy) {
  const maxTokens = strategy.maxTokens || 100000;
  const maxChars = maxTokens * 4;
  if (context.length > maxChars) {
    return context.slice(0, maxChars) + '\n\n[Context truncated...]';
  }
  return context;
}

/**
 * Build execution context for an agent
 * @param {object} params - Context building parameters
 * @param {string} params.id - Agent ID
 * @param {string} params.role - Agent role
 * @param {number} params.iteration - Current iteration number
 * @param {any} params.config - Agent configuration
 * @param {any} params.messageBus - Message bus for querying ledger
 * @param {any} params.cluster - Cluster object
 * @param {number} [params.lastTaskEndTime] - Timestamp of last task completion
 * @param {number} [params.lastAgentStartTime] - Timestamp when this agent last started work
 * @param {any} params.triggeringMessage - Message that triggered this execution
 * @param {string} [params.selectedPrompt] - Pre-selected prompt from _selectPrompt() (iteration-based)
 * @param {object} [params.worktree] - Worktree isolation state (if running in worktree mode)
 * @param {object} [params.isolation] - Docker isolation state (if running in Docker mode)
 * @returns {string} Assembled context string
 */
function buildContext({
  id,
  role,
  iteration,
  config,
  messageBus,
  cluster,
  lastTaskEndTime,
  lastAgentStartTime,
  triggeringMessage,
  selectedPrompt,
  worktree,
  isolation,
}) {
  const strategy = config.contextStrategy || { sources: [] };
  const isIsolated = !!(worktree?.enabled || isolation?.enabled);

  let context = buildHeaderContext({ id, role, iteration, isIsolated });
  context += buildInstructionsSection({ config, selectedPrompt, id });
  context += buildLegacyOutputSchemaSection(config);
  context += buildJsonSchemaSection(config);
  context += buildSourcesSection({
    strategy,
    messageBus,
    cluster,
    lastTaskEndTime,
    lastAgentStartTime,
  });
  context += buildValidatorSkipSection({ role, messageBus, cluster });
  context += buildTriggeringMessageSection(triggeringMessage);

  context = truncateContextIfNeeded(context);
  context = applyLegacyMaxTokens(context, strategy);

  return context;
}

module.exports = {
  buildContext,
};
