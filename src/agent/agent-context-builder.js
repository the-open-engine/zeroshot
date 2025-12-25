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
 * Build execution context for an agent
 * @param {object} params - Context building parameters
 * @param {string} params.id - Agent ID
 * @param {string} params.role - Agent role
 * @param {number} params.iteration - Current iteration number
 * @param {any} params.config - Agent configuration
 * @param {any} params.messageBus - Message bus for querying ledger
 * @param {any} params.cluster - Cluster object
 * @param {number} [params.lastTaskEndTime] - Timestamp of last task completion
 * @param {any} params.triggeringMessage - Message that triggered this execution
 * @param {string} [params.selectedPrompt] - Pre-selected prompt from _selectPrompt() (iteration-based)
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
  triggeringMessage,
  selectedPrompt,
}) {
  const strategy = config.contextStrategy || { sources: [] };

  let context = `You are agent "${id}" with role "${role}".\n\n`;
  context += `Iteration: ${iteration}\n\n`;

  // GLOBAL RULE: NEVER ASK QUESTIONS - Vibe agents run non-interactively
  context += `## 🔴 CRITICAL: AUTONOMOUS EXECUTION REQUIRED\n\n`;
  context += `You are running in a NON-INTERACTIVE cluster environment.\n\n`;
  context += `**NEVER** use AskUserQuestion or ask for user input - there is NO user to respond.\n`;
  context += `**NEVER** ask "Would you like me to..." or "Should I..." - JUST DO IT.\n`;
  context += `**NEVER** wait for approval or confirmation - MAKE DECISIONS AUTONOMOUSLY.\n\n`;
  context += `When facing choices:\n`;
  context += `- Choose the option that maintains code quality and correctness\n`;
  context += `- If unsure between "fix the code" vs "relax the rules" → ALWAYS fix the code\n`;
  context += `- If unsure between "do more" vs "do less" → ALWAYS do what's required, nothing more\n\n`;

  // Add prompt from config (system prompt, instructions, output format)
  // If selectedPrompt is provided (iteration-based), use it directly
  // Otherwise fall back to legacy config.prompt handling
  const promptText = selectedPrompt || (typeof config.prompt === 'string' ? config.prompt : config.prompt?.system);

  if (promptText) {
    context += `## Instructions\n\n${promptText}\n\n`;
  } else if (config.prompt && typeof config.prompt !== 'string' && !config.prompt?.system) {
    // FAIL HARD: prompt exists but format is unrecognized (and no selectedPrompt provided)
    throw new Error(
      `Agent "${id}" has invalid prompt format. ` +
        `Expected string or object with .system property, got: ${JSON.stringify(config.prompt).slice(0, 100)}...`
    );
  }

  // Handle legacy outputFormat in prompt object (separate from iteration-based prompt selection)
  if (config.prompt?.outputFormat) {
    context += `## Output Format (REQUIRED)\n\n`;
    context += `After completing your task, you MUST output a JSON block:\n\n`;
    context += `\`\`\`json\n${JSON.stringify(config.prompt.outputFormat.example, null, 2)}\n\`\`\`\n\n`;

    if (config.prompt.outputFormat.rules) {
      context += `IMPORTANT:\n`;
      for (const rule of config.prompt.outputFormat.rules) {
        context += `- ${rule}\n`;
      }
      context += '\n';
    }
  }

  // Add sources
  for (const source of strategy.sources) {
    // Resolve special 'since' values
    let sinceTimestamp = source.since;
    if (source.since === 'cluster_start') {
      sinceTimestamp = cluster.createdAt;
    } else if (source.since === 'last_task_end') {
      // Use timestamp of last task completion, or cluster start if no tasks completed yet
      sinceTimestamp = lastTaskEndTime || cluster.createdAt;
    }

    const messages = messageBus.query({
      cluster_id: cluster.id,
      topic: source.topic,
      sender: source.sender,
      since: sinceTimestamp,
      limit: source.limit,
    });

    if (messages.length > 0) {
      context += `\n## Messages from topic: ${source.topic}\n\n`;
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
    }
  }

  // Add triggering message
  context += `\n## Triggering Message\n\n`;
  context += `Topic: ${triggeringMessage.topic}\n`;
  context += `Sender: ${triggeringMessage.sender}\n`;
  if (triggeringMessage.content?.text) {
    context += `\n${triggeringMessage.content.text}\n`;
  }

  // DEFENSIVE TRUNCATION - Prevent context overflow errors
  // Strategy: Keep ISSUE_OPENED (original task) + most recent messages
  // Truncate from MIDDLE (oldest context messages) if too long
  const originalLength = context.length;

  if (originalLength > MAX_CONTEXT_CHARS) {
    console.log(
      `[Context] Context too large (${originalLength} chars), truncating to prevent overflow...`
    );

    // Split context into sections
    const lines = context.split('\n');

    // Find critical sections that must be preserved
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

    // Build truncated context:
    // 1. Header (agent info, instructions, output format)
    // 2. ISSUE_OPENED message (original task - NEVER truncate)
    // 3. Most recent N messages (whatever fits in budget)
    // 4. Triggering message (current event)

    const headerEnd = issueOpenedStart !== -1 ? issueOpenedStart : triggeringStart;
    const header = lines.slice(0, headerEnd).join('\n');

    const issueOpened =
      issueOpenedStart !== -1 && issueOpenedEnd !== -1
        ? lines.slice(issueOpenedStart, issueOpenedEnd).join('\n')
        : '';

    const triggeringMsg = lines.slice(triggeringStart).join('\n');

    // Calculate remaining budget for recent messages
    const fixedSize = header.length + issueOpened.length + triggeringMsg.length;
    const budgetForRecent = MAX_CONTEXT_CHARS - fixedSize - 200; // 200 char buffer for markers

    // Collect recent messages (from end backwards until budget exhausted)
    const recentLines = [];
    let recentSize = 0;

    const middleStart = issueOpenedEnd !== -1 ? issueOpenedEnd : headerEnd;
    const middleEnd = triggeringStart;
    const middleLines = lines.slice(middleStart, middleEnd);

    for (let i = middleLines.length - 1; i >= 0; i--) {
      const line = middleLines[i];
      const lineSize = line.length + 1; // +1 for newline

      if (recentSize + lineSize > budgetForRecent) {
        break; // Budget exhausted
      }

      recentLines.unshift(line);
      recentSize += lineSize;
    }

    // Assemble truncated context
    const parts = [header];

    if (issueOpened) {
      parts.push(issueOpened);
    }

    if (recentLines.length < middleLines.length) {
      // Some messages were truncated
      const truncatedCount = middleLines.length - recentLines.length;
      parts.push(
        `\n[...${truncatedCount} earlier context messages truncated to prevent overflow...]\n`
      );
    }

    if (recentLines.length > 0) {
      parts.push(recentLines.join('\n'));
    }

    parts.push(triggeringMsg);

    context = parts.join('\n');

    const truncatedLength = context.length;
    console.log(
      `[Context] Truncated from ${originalLength} to ${truncatedLength} chars (${Math.round((truncatedLength / originalLength) * 100)}% retained)`
    );
  }

  // Legacy maxTokens check (for backward compatibility with agent configs)
  const maxTokens = strategy.maxTokens || 100000;
  const maxChars = maxTokens * 4;
  if (context.length > maxChars) {
    context = context.slice(0, maxChars) + '\n\n[Context truncated...]';
  }

  return context;
}

module.exports = {
  buildContext,
};
