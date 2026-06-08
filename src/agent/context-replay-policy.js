const RAW_LOG_ONLY_REPLAY_POLICY = 'raw_log_only';
const CONTEXT_REPLAY_POLICY = 'context';

function buildRawLogOnlyMetadata(extra = {}) {
  return {
    ...extra,
    contextSafe: false,
    replayPolicy: RAW_LOG_ONLY_REPLAY_POLICY,
  };
}

function getReplayPolicy(message) {
  return message?.metadata?.replayPolicy ?? message?.content?.data?.replayPolicy;
}

function getContextSafe(message) {
  if (typeof message?.metadata?.contextSafe === 'boolean') {
    return message.metadata.contextSafe;
  }

  if (typeof message?.content?.data?.contextSafe === 'boolean') {
    return message.content.data.contextSafe;
  }

  return null;
}

function isReplayableMessage(message) {
  const contextSafe = getContextSafe(message);
  if (contextSafe !== null) {
    return contextSafe;
  }

  const replayPolicy = getReplayPolicy(message);
  if (replayPolicy === RAW_LOG_ONLY_REPLAY_POLICY) {
    return false;
  }

  if (replayPolicy === CONTEXT_REPLAY_POLICY) {
    return true;
  }

  return message?.topic !== 'AGENT_OUTPUT';
}

module.exports = {
  RAW_LOG_ONLY_REPLAY_POLICY,
  CONTEXT_REPLAY_POLICY,
  buildRawLogOnlyMetadata,
  isReplayableMessage,
};
