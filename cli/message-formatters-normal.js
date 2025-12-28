/**
 * Normal mode message formatters
 * Full-detail message display for non-watch mode
 */

const chalk = require('chalk');

/**
 * Format AGENT_LIFECYCLE events
 * @param {Object} msg - Message object
 * @param {string} prefix - Formatted message prefix
 * @returns {boolean} True if message was handled
 */
function formatAgentLifecycle(msg, prefix) {
  const data = msg.content?.data;
  const event = data?.event;

  let icon, eventText;
  switch (event) {
    case 'STARTED':
      icon = chalk.green('▶');
      const triggers = data.triggers?.join(', ') || 'none';
      eventText = `started (listening for: ${chalk.dim(triggers)})`;
      break;
    case 'TASK_STARTED':
      icon = chalk.yellow('⚡');
      eventText = `${chalk.cyan(data.triggeredBy)} → task #${data.iteration} (${chalk.dim(data.model)})`;
      break;
    case 'TASK_COMPLETED':
      icon = chalk.green('✓');
      eventText = `task #${data.iteration} completed`;
      break;
    default:
      icon = chalk.dim('•');
      eventText = event || 'unknown event';
  }

  console.log(`${prefix} ${icon} ${eventText}`);
  return true;
}

/**
 * Format AGENT_ERROR events
 * @param {Object} msg - Message object
 * @param {string} prefix - Formatted message prefix
 * @param {string} timestamp - Formatted timestamp
 * @returns {boolean} True if message was handled
 */
function formatAgentError(msg, prefix, timestamp) {
  console.log(''); // Blank line before error
  console.log(chalk.bold.red(`${'─'.repeat(60)}`));
  console.log(`${prefix} ${chalk.gray(timestamp)} ${chalk.bold.red('🔴 AGENT ERROR')}`);

  if (msg.content?.text) {
    console.log(`${prefix} ${chalk.red(msg.content.text)}`);
  }

  if (msg.content?.data?.stack) {
    const stackLines = msg.content.data.stack.split('\n').slice(0, 5);
    for (const line of stackLines) {
      if (line.trim()) {
        console.log(`${prefix} ${chalk.dim(line)}`);
      }
    }
  }

  console.log(chalk.bold.red(`${'─'.repeat(60)}`));
  return true;
}

/**
 * Format ISSUE_OPENED events
 * @param {Object} msg - Message object
 * @param {string} prefix - Formatted message prefix
 * @param {string} timestamp - Formatted timestamp
 * @param {Set} shownNewTaskForCluster - Set tracking shown tasks
 * @returns {boolean} True if message was handled
 */
function formatIssueOpened(msg, prefix, timestamp, shownNewTaskForCluster) {
  // Skip duplicate - conductor re-publishes after spawning agents
  if (shownNewTaskForCluster.has(msg.cluster_id)) {
    return true;
  }
  shownNewTaskForCluster.add(msg.cluster_id);

  console.log(''); // Blank line before new task
  console.log(chalk.bold.blue(`${'─'.repeat(60)}`));
  console.log(`${prefix} ${chalk.gray(timestamp)} ${chalk.bold.blue('📋 NEW TASK')}`);

  if (msg.content?.text) {
    const lines = msg.content.text.split('\n').slice(0, 3);
    for (const line of lines) {
      if (line.trim() && line.trim() !== '# Manual Input') {
        console.log(`${prefix} ${chalk.white(line)}`);
      }
    }
  }

  console.log(chalk.bold.blue(`${'─'.repeat(60)}`));
  return true;
}

/**
 * Format IMPLEMENTATION_READY events
 * @param {Object} msg - Message object
 * @param {string} prefix - Formatted message prefix
 * @param {string} timestamp - Formatted timestamp
 * @returns {boolean} True if message was handled
 */
function formatImplementationReady(msg, prefix, timestamp) {
  console.log(
    `${prefix} ${chalk.gray(timestamp)} ${chalk.bold.yellow('✅ IMPLEMENTATION READY')}`
  );

  if (msg.content?.data?.commit) {
    console.log(
      `${prefix} ${chalk.gray('Commit:')} ${chalk.cyan(msg.content.data.commit.substring(0, 8))}`
    );
  }

  return true;
}

/**
 * Format VALIDATION_RESULT events
 * @param {Object} msg - Message object
 * @param {string} prefix - Formatted message prefix
 * @param {string} timestamp - Formatted timestamp
 * @returns {boolean} True if message was handled
 */
function formatValidationResult(msg, prefix, timestamp) {
  const data = msg.content?.data || {};
  const approved = data.approved === true || data.approved === 'true';
  const status = approved ? chalk.bold.green('✓ APPROVED') : chalk.bold.red('✗ REJECTED');

  console.log(`${prefix} ${chalk.gray(timestamp)} ${status}`);

  // Show summary if present and not a template variable
  if (msg.content?.text && !msg.content.text.includes('{{')) {
    console.log(`${prefix} ${msg.content.text.substring(0, 100)}`);
  }

  // Show full JSON data structure
  console.log(
    `${prefix} ${chalk.dim(JSON.stringify(data, null, 2).split('\n').join(`\n${prefix} `))}`
  );

  return true;
}

/**
 * Format CLUSTER_COMPLETE events
 * @param {Object} msg - Message object
 * @param {string} prefix - Formatted message prefix
 * @param {string} timestamp - Formatted timestamp
 * @returns {boolean} True if message was handled
 */
function formatClusterComplete(msg, prefix, timestamp) {
  console.log(''); // Blank line
  console.log(chalk.bold.green(`${'═'.repeat(60)}`));
  console.log(`${prefix} ${chalk.gray(timestamp)} ${chalk.bold.green('🎉 CLUSTER COMPLETE')}`);
  if (msg.content?.data?.reason) {
    console.log(`${prefix} ${chalk.green(msg.content.data.reason)}`);
  }
  console.log(chalk.bold.green(`${'═'.repeat(60)}`));
  return true;
}

/**
 * Format CLUSTER_FAILED events
 * @param {Object} msg - Message object
 * @param {string} prefix - Formatted message prefix
 * @param {string} timestamp - Formatted timestamp
 * @returns {boolean} True if message was handled
 */
function formatClusterFailed(msg, prefix, timestamp) {
  console.log(''); // Blank line
  console.log(chalk.bold.red(`${'═'.repeat(60)}`));
  console.log(`${prefix} ${chalk.gray(timestamp)} ${chalk.bold.red('❌ CLUSTER FAILED')}`);
  if (msg.content?.text) {
    console.log(`${prefix} ${chalk.red(msg.content.text)}`);
  }
  if (msg.content?.data?.reason) {
    console.log(`${prefix} ${chalk.red(msg.content.data.reason)}`);
  }
  console.log(chalk.bold.red(`${'═'.repeat(60)}`));
  return true;
}

/**
 * Format generic messages (fallback)
 * @param {Object} msg - Message object
 * @param {string} prefix - Formatted message prefix
 * @param {string} timestamp - Formatted timestamp
 * @returns {boolean} True if message was handled
 */
function formatGenericMessage(msg, prefix, timestamp) {
  console.log(`${prefix} ${chalk.gray(timestamp)} ${chalk.bold(msg.topic)}`);
  if (msg.content?.text) {
    console.log(`${prefix} ${msg.content.text}`);
  }
  return true;
}

module.exports = {
  formatAgentLifecycle,
  formatAgentError,
  formatIssueOpened,
  formatImplementationReady,
  formatValidationResult,
  formatClusterComplete,
  formatClusterFailed,
  formatGenericMessage,
};
