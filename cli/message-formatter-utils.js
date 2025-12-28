/**
 * Message formatting utilities for CLI output
 * Extracted from index.js to reduce complexity
 */

const chalk = require('chalk');

/**
 * Get color for sender based on consistent hashing
 * @param {string} sender - Sender name
 * @returns {Function} Chalk color function
 */
function getColorForSender(sender) {
  const colors = [chalk.cyan, chalk.magenta, chalk.yellow, chalk.green, chalk.blue];
  let hash = 0;
  for (let i = 0; i < sender.length; i++) {
    hash = (hash << 5) - hash + sender.charCodeAt(i);
    hash = hash & hash;
  }
  return colors[Math.abs(hash) % colors.length];
}

/**
 * Build message prefix with timestamp, sender, and optional cluster ID
 * @param {Object} msg - Message object
 * @param {boolean} showClusterId - Whether to show cluster ID
 * @param {boolean} isActive - Whether cluster is active
 * @returns {string} Formatted prefix
 */
function buildMessagePrefix(msg, showClusterId, isActive) {
  const color = isActive ? getColorForSender(msg.sender) : chalk.dim;

  let senderLabel = msg.sender;
  if (showClusterId && msg.cluster_id) {
    senderLabel = `${msg.cluster_id}/${msg.sender}`;
  }

  const modelSuffix = msg.sender_model ? chalk.dim(` [${msg.sender_model}]`) : '';
  return color(`${senderLabel.padEnd(showClusterId ? 25 : 15)} |`) + modelSuffix;
}

/**
 * Build cluster prefix for watch mode
 * @param {string} clusterId - Cluster ID
 * @param {boolean} isActive - Whether cluster is active
 * @returns {string} Formatted prefix
 */
function buildClusterPrefix(clusterId, isActive) {
  return isActive
    ? chalk.white(`${clusterId.padEnd(20)} |`)
    : chalk.dim(`${clusterId.padEnd(20)} |`);
}

/**
 * Parse and normalize data fields (handles string JSON)
 * @param {string|Array} data - Data to parse
 * @returns {Array} Parsed array
 */
function parseDataField(data) {
  if (typeof data === 'string') {
    try {
      return JSON.parse(data);
    } catch {
      return [];
    }
  }
  return Array.isArray(data) ? data : [];
}

module.exports = {
  getColorForSender,
  buildMessagePrefix,
  buildClusterPrefix,
  parseDataField,
};
