/**
 * AgentInputInjector - Live guidance injection for running agents
 *
 * Resolves the current task, validates attachable socket availability,
 * and sends input via attach STDIN when possible.
 */

const { getTask } = require('../../task-lib/store.js');
const { sendInput } = require('../attach/send-input');
const { isSocketAlive } = require('../attach/socket-discovery');

const DEFAULT_TIMEOUT_MS = 1500;

function buildResult({ status, reason = null, method = null, taskId = null }) {
  return {
    status,
    reason,
    method,
    taskId,
  };
}

function ensureValidInputs(agent, text) {
  if (!agent) {
    throw new Error('AgentInputInjector: agent is required');
  }
  if (typeof text !== 'string') {
    throw new Error('AgentInputInjector: text must be a string');
  }
  if (!text.trim()) {
    throw new Error('AgentInputInjector: text cannot be empty');
  }
}

function buildUnsupported(reason, taskId) {
  return buildResult({
    status: 'unsupported',
    reason,
    taskId,
  });
}

function getTaskId(agent) {
  return agent.currentTaskId || null;
}

function checkIsolation(agent, taskId) {
  if (agent.isolation?.enabled) {
    return buildUnsupported('isolation-enabled', taskId);
  }
  return null;
}

function checkTaskId(taskId) {
  if (!taskId) {
    return buildUnsupported('no-current-task', null);
  }
  return null;
}

function checkTaskInfo(taskInfo, taskId) {
  if (!taskInfo) {
    return buildUnsupported('task-not-found', taskId);
  }
  if (!taskInfo.socketPath) {
    return buildUnsupported('no-socket', taskId);
  }
  if (!taskInfo.attachable) {
    return buildUnsupported('task-not-attachable', taskId);
  }
  return null;
}

async function checkSocketAlive(socketPath, taskId) {
  const socketAlive = await isSocketAlive(socketPath);
  if (!socketAlive) {
    return buildUnsupported('socket-not-alive', taskId);
  }
  return null;
}

function normalizePayload(text) {
  return text.endsWith('\n') ? text : `${text}\n`;
}

function resolveTimeout(options) {
  return options.timeoutMs || DEFAULT_TIMEOUT_MS;
}

function buildInjected(taskId) {
  return buildResult({
    status: 'injected',
    method: 'pty',
    taskId,
  });
}

function buildSendFailure(reason, taskId) {
  return buildResult({
    status: 'unsupported',
    reason: reason || 'send-failed',
    method: 'pty',
    taskId,
  });
}

async function injectInput(agent, text, options = {}) {
  ensureValidInputs(agent, text);

  const taskId = getTaskId(agent);
  const isolationResult = checkIsolation(agent, taskId);
  if (isolationResult) return isolationResult;

  const taskIdResult = checkTaskId(taskId);
  if (taskIdResult) return taskIdResult;

  const taskInfo = getTask(taskId);
  const taskInfoResult = checkTaskInfo(taskInfo, taskId);
  if (taskInfoResult) return taskInfoResult;

  const socketResult = await checkSocketAlive(taskInfo.socketPath, taskId);
  if (socketResult) return socketResult;

  const payload = normalizePayload(text);
  const timeoutMs = resolveTimeout(options);
  const result = await sendInput({
    socketPath: taskInfo.socketPath,
    data: payload,
    timeoutMs,
  });

  if (!result.ok) {
    return buildSendFailure(result.error, taskId);
  }

  return buildInjected(taskId);
}

module.exports = {
  injectInput,
};
