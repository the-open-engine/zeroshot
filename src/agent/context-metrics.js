const TOKENS_PER_CHAR_ESTIMATE = 4;

function estimateTokensFromChars(chars) {
  if (!Number.isFinite(chars) || chars <= 0) {
    return 0;
  }

  return Math.ceil(chars / TOKENS_PER_CHAR_ESTIMATE);
}

function buildSectionMetrics(sections) {
  const sectionMetrics = {};
  let totalChars = 0;

  for (const [sectionName, text] of Object.entries(sections)) {
    const safeText = typeof text === 'string' ? text : '';
    const chars = safeText.length;
    const estimatedTokens = estimateTokensFromChars(chars);
    sectionMetrics[sectionName] = { chars, estimatedTokens };
    totalChars += chars;
  }

  return { sectionMetrics, totalChars };
}

function resolveLegacyMaxTokens(strategy) {
  if (!strategy) {
    return 100000;
  }

  return strategy.maxTokens || 100000;
}

function buildContextMetrics({
  clusterId,
  agentId,
  role,
  iteration,
  triggeringMessage,
  strategy,
  sections,
}) {
  const { sectionMetrics, totalChars } = buildSectionMetrics(sections);
  const maxTokens = resolveLegacyMaxTokens(strategy);
  const sourcesCount = Array.isArray(strategy?.sources) ? strategy.sources.length : 0;

  return {
    clusterId,
    agentId,
    role,
    iteration,
    triggeredBy: triggeringMessage?.topic || null,
    triggerFrom: triggeringMessage?.sender || null,
    strategy: {
      maxTokens,
      sourcesCount,
    },
    sections: sectionMetrics,
    total: {
      chars: totalChars,
      estimatedTokens: estimateTokensFromChars(totalChars),
    },
    truncation: {
      maxContextChars: {
        applied: false,
        beforeChars: totalChars,
        afterChars: totalChars,
      },
      legacyMaxTokens: {
        applied: false,
        beforeChars: totalChars,
        afterChars: totalChars,
        maxTokens,
      },
    },
  };
}

function updateTotalMetrics(metrics, chars) {
  if (!metrics || !Number.isFinite(chars)) {
    return;
  }

  metrics.total = {
    chars,
    estimatedTokens: estimateTokensFromChars(chars),
  };
}

function emitContextMetrics(metrics, { messageBus, clusterId, agentId }) {
  if (process.env.ZEROSHOT_CONTEXT_METRICS === '1') {
    console.log('[ContextMetrics]', JSON.stringify(metrics));
  }

  if (process.env.ZEROSHOT_CONTEXT_METRICS_LEDGER === '1' && messageBus?.publish) {
    messageBus.publish({
      cluster_id: clusterId,
      topic: 'CONTEXT_METRICS',
      sender: agentId,
      receiver: 'system',
      content: {
        data: metrics,
      },
    });
  }
}

module.exports = {
  estimateTokensFromChars,
  resolveLegacyMaxTokens,
  buildContextMetrics,
  updateTotalMetrics,
  emitContextMetrics,
};
