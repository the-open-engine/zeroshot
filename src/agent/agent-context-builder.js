/**
 * AgentContextBuilder - Build agent execution context from ledger
 *
 * Provides:
 * - Context assembly from multiple message sources
 * - Context strategy evaluation (topics, limits, since timestamps)
 * - Prompt injection and formatting
 * - Token-budgeted context packs
 * - Defensive context overflow prevention
 */

// Defensive limit: 500,000 chars ≈ 125k tokens (safe buffer below 200k limit)
// Prevents "Prompt is too long" errors that kill tasks
const MAX_CONTEXT_CHARS = 500000;
const {
  buildContextMetrics,
  emitContextMetrics,
  resolveLegacyMaxTokens,
  updateTotalMetrics,
} = require('./context-metrics');
const { buildContextPacks } = require('./context-pack-builder');

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
        `Unknown context source "since" value "${sinceValue}" for topic ${source.topic}. ` +
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

function resolveSourceSelection(source, { compact = false } = {}) {
  const baseAmount = source.amount ?? source.limit;
  const baseStrategy = source.strategy ?? (baseAmount !== undefined ? 'latest' : 'all');

  if (!compact) {
    return { amount: baseAmount, strategy: baseStrategy };
  }

  const compactAmount = source.compactAmount ?? (baseAmount !== undefined ? 1 : 1);
  const compactStrategy =
    source.compactStrategy ?? (baseStrategy === 'all' ? 'latest' : baseStrategy);

  return { amount: compactAmount, strategy: compactStrategy };
}

function resolveSourceMessages({
  source,
  messageBus,
  cluster,
  lastTaskEndTime,
  lastAgentStartTime,
  compact = false,
}) {
  const sinceTimestamp = resolveSourceSince(source, cluster, lastTaskEndTime, lastAgentStartTime);
  const { amount, strategy } = resolveSourceSelection(source, { compact });
  const order = strategy === 'latest' ? 'desc' : 'asc';
  const messages = messageBus.query({
    cluster_id: cluster.id,
    topic: source.topic,
    sender: source.sender,
    since: sinceTimestamp,
    limit: amount,
    order,
  });

  if (strategy !== 'latest' || messages.length <= 1) {
    return messages;
  }

  return messages.slice().reverse();
}

function resolveSourcePriority(source) {
  if (source.priority) {
    return source.priority;
  }
  if (source.topic === 'STATE_SNAPSHOT') {
    return 'required';
  }
  if (source.topic === 'ISSUE_OPENED' || source.topic === 'PLAN_READY') {
    return 'required';
  }
  if (source.topic === 'VALIDATION_RESULT' || source.topic === 'IMPLEMENTATION_READY') {
    return 'high';
  }
  return 'medium';
}

function buildSourcePack({
  source,
  index,
  messageBus,
  cluster,
  lastTaskEndTime,
  lastAgentStartTime,
}) {
  const packId = `source:${source.topic}:${index}`;
  const priority = resolveSourcePriority(source);

  const render = (compact) => {
    const messages = resolveSourceMessages({
      source,
      messageBus,
      cluster,
      lastTaskEndTime,
      lastAgentStartTime,
      compact,
    });
    if (messages.length === 0) return '';
    return formatSourceMessagesSection(source, messages);
  };

  return {
    id: packId,
    section: 'sources',
    priority,
    render: () => render(false),
    compact: () => render(true),
  };
}

const { isPlatformMismatchReason } = require('./validation-platform');

function collectCannotValidateCriteria(prevValidations, options = {}) {
  const cannotValidateCriteria = [];
  const ignoreReason = options.ignoreReason;
  for (const msg of prevValidations) {
    const criteriaResults = msg.content?.data?.criteriaResults;
    if (!Array.isArray(criteriaResults)) continue;
    for (const cr of criteriaResults) {
      if (cr.status !== 'CANNOT_VALIDATE' || !cr.id) continue;
      if (ignoreReason && ignoreReason(cr.reason)) continue;
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

function buildValidatorSkipSection({ role, messageBus, cluster, isolation }) {
  if (role !== 'validator') return '';

  const prevValidations = messageBus.query({
    cluster_id: cluster.id,
    topic: 'VALIDATION_RESULT',
    since: cluster.createdAt,
    limit: 50,
  });

  const ignoreReason = isolation?.enabled ? isPlatformMismatchReason : null;
  const cannotValidateCriteria = collectCannotValidateCriteria(prevValidations, { ignoreReason });
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

  const header = buildHeaderContext({ id, role, iteration, isIsolated });
  const instructions = buildInstructionsSection({ config, selectedPrompt, id });
  const legacyOutputSchema = buildLegacyOutputSchemaSection(config);
  const jsonSchema = buildJsonSchemaSection(config);
  const validatorSkip = buildValidatorSkipSection({ role, messageBus, cluster, isolation });
  const triggeringMessageSection = buildTriggeringMessageSection(triggeringMessage);

  const packs = [];
  let order = 0;

  const pushStaticPack = (packId, section, text, options = {}) => {
    if (!text) return;
    packs.push({
      id: packId,
      section,
      priority: 'required',
      order: order++,
      preserve: options.preserve || false,
      render: () => text,
    });
  };

  pushStaticPack('header', 'header', header);
  pushStaticPack('instructions', 'instructions', instructions);
  pushStaticPack('legacyOutputSchema', 'legacyOutputSchema', legacyOutputSchema);
  pushStaticPack('jsonSchema', 'jsonSchema', jsonSchema);

  if (Array.isArray(strategy.sources)) {
    strategy.sources.forEach((source, index) => {
      const pack = buildSourcePack({
        source,
        index,
        messageBus,
        cluster,
        lastTaskEndTime,
        lastAgentStartTime,
      });
      packs.push({ ...pack, order: order++ });
    });
  }

  pushStaticPack('validatorSkip', 'validatorSkip', validatorSkip);
  pushStaticPack('triggeringMessage', 'triggeringMessage', triggeringMessageSection, {
    preserve: true,
  });

  const maxTokens = resolveLegacyMaxTokens(strategy);
  const packResult = buildContextPacks({
    packs,
    maxTokens,
    maxChars: MAX_CONTEXT_CHARS,
  });

  const metrics = buildContextMetrics({
    clusterId: cluster.id,
    agentId: id,
    role,
    iteration,
    triggeringMessage,
    strategy,
    packs: packResult.packDecisions,
    budget: packResult.budget,
    truncation: packResult.truncation,
  });

  updateTotalMetrics(metrics, packResult.context.length);
  emitContextMetrics(metrics, { messageBus, clusterId: cluster.id, agentId: id });

  return packResult.context;
}

module.exports = {
  buildContext,
};
