/**
 * Watch mode message formatters
 * Simplified, high-level event display for zeroshot watch command
 */

const chalk = require('chalk');
const { buildClusterPrefix, getColorForSender, parseDataField } = require('./message-formatter-utils');

/**
 * Format AGENT_ERROR for watch mode
 * @param {Object} msg - Message object
 * @param {string} clusterPrefix - Formatted cluster prefix
 */
function formatAgentError(msg, clusterPrefix) {
  const errorMsg = `${msg.sender} ${chalk.bold.red('ERROR')}`;
  console.log(`${clusterPrefix} ${errorMsg}`);
  if (msg.content?.text) {
    console.log(`${clusterPrefix}   ${chalk.red(msg.content.text)}`);
  }
}

/**
 * Format ISSUE_OPENED for watch mode
 * @param {Object} msg - Message object
 * @param {string} clusterPrefix - Formatted cluster prefix
 */
function formatIssueOpened(msg, clusterPrefix) {
  const issueNum = msg.content?.data?.issue_number || '';
  const title = msg.content?.data?.title || '';
  const prompt = msg.content?.data?.prompt || msg.content?.text || '';

  const taskDesc = title === 'Manual Input' && prompt ? prompt : title;
  const truncatedDesc =
    taskDesc && taskDesc.length > 60 ? taskDesc.substring(0, 60) + '...' : taskDesc;

  const eventText = `Started ${issueNum ? `#${issueNum}` : 'task'}${truncatedDesc ? chalk.dim(` - ${truncatedDesc}`) : ''}`;
  console.log(`${clusterPrefix} ${eventText}`);
}

/**
 * Format IMPLEMENTATION_READY for watch mode
 * @param {Object} msg - Message object
 * @param {string} clusterPrefix - Formatted cluster prefix
 */
function formatImplementationReady(msg, clusterPrefix) {
  const agentColor = getColorForSender(msg.sender);
  const agentName = agentColor(msg.sender);
  const eventText = `${agentName} completed implementation`;
  console.log(`${clusterPrefix} ${eventText}`);
}

/**
 * Format VALIDATION_RESULT for watch mode
 * @param {Object} msg - Message object
 * @param {string} clusterPrefix - Formatted cluster prefix
 */
function formatValidationResult(msg, clusterPrefix) {
  const agentColor = getColorForSender(msg.sender);
  const agentName = agentColor(msg.sender);
  const data = msg.content?.data;
  const approved = data?.approved === 'true' || data?.approved === true;
  const status = approved ? chalk.green('APPROVED') : chalk.red('REJECTED');

  let eventText = `${agentName} ${status}`;
  if (data?.summary && !approved) {
    eventText += chalk.dim(` - ${data.summary}`);
  }
  console.log(`${clusterPrefix} ${eventText}`);

  if (!approved) {
    printRejectionDetails(data, clusterPrefix);
  }
}

/**
 * Print rejection details (errors/issues)
 * @param {Object} data - Validation data
 * @param {string} clusterPrefix - Formatted cluster prefix
 */
function printRejectionDetails(data, clusterPrefix) {
  const errors = parseDataField(data.errors);
  const issues = parseDataField(data.issues);

  if (errors.length > 0) {
    const errorsCharCount = JSON.stringify(errors).length;
    console.log(
      `${clusterPrefix}   ${chalk.red('•')} ${errors.length} error${errors.length > 1 ? 's' : ''} (${errorsCharCount} chars)`
    );
  }

  if (issues.length > 0) {
    const issuesCharCount = JSON.stringify(issues).length;
    console.log(
      `${clusterPrefix}   ${chalk.yellow('•')} ${issues.length} issue${issues.length > 1 ? 's' : ''} (${issuesCharCount} chars)`
    );
  }
}

/**
 * Format PR_CREATED for watch mode
 * @param {Object} msg - Message object
 * @param {string} clusterPrefix - Formatted cluster prefix
 */
function formatPrCreated(msg, clusterPrefix) {
  const agentColor = getColorForSender(msg.sender);
  const agentName = agentColor(msg.sender);
  const prNum = msg.content?.data?.pr_number || '';
  const eventText = `${agentName} created PR${prNum ? ` #${prNum}` : ''}`;
  console.log(`${clusterPrefix} ${eventText}`);
}

/**
 * Format PR_MERGED for watch mode
 * @param {Object} msg - Message object
 * @param {string} clusterPrefix - Formatted cluster prefix
 */
function formatPrMerged(msg, clusterPrefix) {
  const agentColor = getColorForSender(msg.sender);
  const agentName = agentColor(msg.sender);
  const eventText = `${agentName} merged PR`;
  console.log(`${clusterPrefix} ${eventText}`);
}

/**
 * Format unknown topic for watch mode (fallback)
 * @param {Object} msg - Message object
 * @param {string} clusterPrefix - Formatted cluster prefix
 */
function formatUnknownTopic(msg, clusterPrefix) {
  const agentColor = getColorForSender(msg.sender);
  const agentName = agentColor(msg.sender);
  const eventText = `${agentName} ${msg.topic.toLowerCase().replace(/_/g, ' ')}`;
  console.log(`${clusterPrefix} ${eventText}`);
}

/**
 * Main watch mode formatter
 * @param {Object} msg - Message object
 * @param {boolean} isActive - Whether cluster is active
 * @returns {boolean} True if message was handled
 */
function formatWatchMode(msg, isActive) {
  // Skip low-level topics (too noisy for watch mode)
  if (msg.topic === 'AGENT_OUTPUT' || msg.topic === 'AGENT_LIFECYCLE') {
    return true;
  }

  // Clear status line, print message, will be redrawn by status interval
  process.stdout.write('\r' + ' '.repeat(120) + '\r');

  const clusterPrefix = buildClusterPrefix(msg.cluster_id, isActive);

  switch (msg.topic) {
    case 'AGENT_ERROR':
      formatAgentError(msg, clusterPrefix);
      break;
    case 'ISSUE_OPENED':
      formatIssueOpened(msg, clusterPrefix);
      break;
    case 'IMPLEMENTATION_READY':
      formatImplementationReady(msg, clusterPrefix);
      break;
    case 'VALIDATION_RESULT':
      formatValidationResult(msg, clusterPrefix);
      break;
    case 'PR_CREATED':
      formatPrCreated(msg, clusterPrefix);
      break;
    case 'PR_MERGED':
      formatPrMerged(msg, clusterPrefix);
      break;
    default:
      formatUnknownTopic(msg, clusterPrefix);
  }

  return true;
}

module.exports = {
  formatWatchMode,
};
