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

function buildSectionMetricsFromPacks(packs) {
  const sectionMetrics = {};
  let totalChars = 0;

  for (const pack of packs) {
    if (pack.status !== 'included') continue;
    const sectionName = pack.section || pack.id || 'unknown';
    const chars = Number.isFinite(pack.chars) ? pack.chars : 0;
    if (!sectionMetrics[sectionName]) {
      sectionMetrics[sectionName] = { chars: 0, estimatedTokens: 0 };
    }
    sectionMetrics[sectionName].chars += chars;
    totalChars += chars;
  }

  for (const section of Object.values(sectionMetrics)) {
    section.estimatedTokens = estimateTokensFromChars(section.chars);
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
  packs,
  budget,
  truncation,
}) {
  const maxTokens = resolveLegacyMaxTokens(strategy);
  const sourcesCount = Array.isArray(strategy?.sources) ? strategy.sources.length : 0;
  const packMetrics = Array.isArray(packs) ? packs : [];

  let sectionMetrics = {};
  let totalChars = 0;
  if (packMetrics.length > 0) {
    const packTotals = buildSectionMetricsFromPacks(packMetrics);
    sectionMetrics = packTotals.sectionMetrics;
    totalChars = packTotals.totalChars;
  } else if (sections) {
    const sectionTotals = buildSectionMetrics(sections);
    sectionMetrics = sectionTotals.sectionMetrics;
    totalChars = sectionTotals.totalChars;
  }

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
    budget: {
      maxTokens: budget?.maxTokens ?? maxTokens,
      remainingTokens: budget?.remainingTokens === undefined ? null : budget?.remainingTokens,
      overBudgetTokens: budget?.overBudgetTokens ?? 0,
      finalTokens: budget?.finalTokens ?? estimateTokensFromChars(totalChars),
    },
    packs: packMetrics,
    sections: sectionMetrics,
    total: {
      chars: totalChars,
      estimatedTokens: estimateTokensFromChars(totalChars),
    },
    truncation: {
      maxContextChars: truncation?.maxContextChars || {
        applied: false,
        beforeChars: totalChars,
        afterChars: totalChars,
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

  if (metrics.budget) {
    metrics.budget.finalTokens = estimateTokensFromChars(chars);
  }

  if (metrics.truncation?.maxContextChars) {
    metrics.truncation.maxContextChars.afterChars = chars;
  }
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
