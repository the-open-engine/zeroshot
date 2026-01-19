const { estimateTokensFromChars } = require('./context-metrics');

const PRIORITY_RANK = {
  required: 0,
  high: 1,
  medium: 2,
  low: 3,
};

const DEFAULT_PRIORITY = 'medium';
const TRUNCATION_SUFFIX = '\n\n[Context truncated to fit limit]\n';

function normalizePriority(priority, required) {
  if (required) return 'required';
  if (priority && PRIORITY_RANK[priority] !== undefined) return priority;
  return DEFAULT_PRIORITY;
}

function normalizePack(pack, index) {
  const priority = normalizePriority(pack.priority, pack.required);
  return {
    ...pack,
    priority,
    required: pack.required || priority === 'required',
    order: pack.order ?? index,
  };
}

function renderVariant(pack, variant, cache) {
  const cacheKey = `${pack.id}:${variant}`;
  if (cache.has(cacheKey)) return cache.get(cacheKey);

  let text = '';
  if (variant === 'full') {
    text = typeof pack.render === 'function' ? pack.render() : '';
  } else if (variant === 'compact') {
    text = typeof pack.compact === 'function' ? pack.compact() : '';
  }

  if (typeof text !== 'string') {
    text = '';
  }

  const chars = text.length;
  const estimatedTokens = estimateTokensFromChars(chars);
  const rendered = { text, chars, estimatedTokens };
  cache.set(cacheKey, rendered);
  return rendered;
}

function sortByPriorityThenOrder(a, b) {
  const priorityDelta = PRIORITY_RANK[a.priority] - PRIORITY_RANK[b.priority];
  if (priorityDelta !== 0) return priorityDelta;
  return a.order - b.order;
}

function sortByOrder(a, b) {
  return a.order - b.order;
}

function sortByPriorityDescThenOrderDesc(a, b) {
  const priorityDelta = PRIORITY_RANK[b.priority] - PRIORITY_RANK[a.priority];
  if (priorityDelta !== 0) return priorityDelta;
  return b.order - a.order;
}

function selectVariant(pack, remainingTokens, cache) {
  const full = renderVariant(pack, 'full', cache);
  const compact = pack.compact ? renderVariant(pack, 'compact', cache) : null;

  const hasFull = full.chars > 0;
  const hasCompact = compact && compact.chars > 0;

  if (!hasFull && !hasCompact) {
    return {
      status: 'skipped',
      variant: null,
      reason: 'empty',
      chars: 0,
      estimatedTokens: 0,
    };
  }

  if (pack.required) {
    let chosen = full;
    let variant = 'full';

    if (!hasFull && hasCompact) {
      chosen = compact;
      variant = 'compact';
    } else if (
      Number.isFinite(remainingTokens) &&
      hasCompact &&
      full.estimatedTokens > remainingTokens
    ) {
      if (
        compact.estimatedTokens <= remainingTokens ||
        compact.estimatedTokens < full.estimatedTokens
      ) {
        chosen = compact;
        variant = 'compact';
      }
    }

    return {
      status: 'included',
      variant,
      chars: chosen.chars,
      estimatedTokens: chosen.estimatedTokens,
      text: chosen.text,
    };
  }

  if (!Number.isFinite(remainingTokens) || full.estimatedTokens <= remainingTokens) {
    return {
      status: 'included',
      variant: 'full',
      chars: full.chars,
      estimatedTokens: full.estimatedTokens,
      text: full.text,
    };
  }

  if (hasCompact && compact.estimatedTokens <= remainingTokens) {
    return {
      status: 'included',
      variant: 'compact',
      chars: compact.chars,
      estimatedTokens: compact.estimatedTokens,
      text: compact.text,
    };
  }

  return {
    status: 'skipped',
    variant: null,
    reason: 'budget',
    chars: 0,
    estimatedTokens: 0,
  };
}

function truncateText(text, targetChars) {
  if (text.length <= targetChars) {
    return { text, truncated: false };
  }

  if (targetChars <= 0) {
    return { text: '', truncated: true };
  }

  if (targetChars <= TRUNCATION_SUFFIX.length) {
    return { text: text.slice(0, targetChars), truncated: true };
  }

  const sliceLength = targetChars - TRUNCATION_SUFFIX.length;
  return { text: text.slice(0, sliceLength) + TRUNCATION_SUFFIX, truncated: true };
}

function applyMaxCharsGuard({ packs, selected, decisions, cache, maxChars, totalChars }) {
  let currentChars = totalChars;
  if (!Number.isFinite(maxChars) || currentChars <= maxChars) {
    return { applied: false, beforeChars: totalChars, afterChars: totalChars };
  }

  const includedOptional = packs
    .filter((pack) => selected.has(pack.id) && !pack.required)
    .sort(sortByPriorityDescThenOrderDesc);

  for (const pack of includedOptional) {
    if (currentChars <= maxChars) break;
    const decision = decisions.get(pack.id);
    if (!decision || decision.variant === 'compact' || !pack.compact) continue;

    const compact = renderVariant(pack, 'compact', cache);
    if (compact.chars === 0 || compact.chars >= decision.chars) continue;

    const previousChars = decision.chars;
    selected.set(pack.id, { ...compact, variant: 'compact' });
    decision.variant = 'compact';
    decision.chars = compact.chars;
    decision.estimatedTokens = compact.estimatedTokens;
    decision.reason = decision.reason || 'max_chars';

    currentChars -= previousChars - compact.chars;
    currentChars = Math.max(0, currentChars);
  }

  for (const pack of includedOptional) {
    if (currentChars <= maxChars) break;
    if (!selected.has(pack.id)) continue;

    const decision = decisions.get(pack.id);
    currentChars -= decision?.chars || 0;
    selected.delete(pack.id);
    if (decision) {
      decision.status = 'skipped';
      decision.reason = decision.reason || 'max_chars';
      decision.chars = 0;
      decision.estimatedTokens = 0;
    }
  }

  if (currentChars > maxChars) {
    let overage = currentChars - maxChars;
    const requiredCandidates = packs
      .filter((pack) => selected.has(pack.id) && pack.required)
      .sort((a, b) => {
        const preserveDelta = (a.preserve ? 1 : 0) - (b.preserve ? 1 : 0);
        if (preserveDelta !== 0) return preserveDelta;
        const sizeDelta = (selected.get(b.id)?.chars || 0) - (selected.get(a.id)?.chars || 0);
        if (sizeDelta !== 0) return sizeDelta;
        return b.order - a.order;
      });

    for (const pack of requiredCandidates) {
      if (overage <= 0) break;
      const decision = decisions.get(pack.id);
      const selectedPack = selected.get(pack.id);
      if (!decision || !selectedPack) continue;

      const targetChars = Math.max(0, selectedPack.chars - overage);
      const truncated = truncateText(selectedPack.text, targetChars);
      if (truncated.text.length === selectedPack.chars) continue;

      const newChars = truncated.text.length;
      const reduced = selectedPack.chars - newChars;
      overage -= reduced;

      selected.set(pack.id, {
        text: truncated.text,
        chars: newChars,
        estimatedTokens: estimateTokensFromChars(newChars),
        variant: selectedPack.variant,
      });

      decision.chars = newChars;
      decision.estimatedTokens = estimateTokensFromChars(newChars);
      decision.truncated = true;
      decision.reason = decision.reason || 'max_chars';
    }
  }

  const afterChars = Array.from(selected.values()).reduce((sum, item) => sum + item.chars, 0);
  return { applied: true, beforeChars: totalChars, afterChars };
}

function buildContextPacks({ packs, maxTokens, maxChars }) {
  const normalized = packs.map(normalizePack);
  const selectionOrder = normalized.slice().sort(sortByPriorityThenOrder);
  const renderCache = new Map();
  const decisions = new Map();
  const selected = new Map();

  let remainingTokens = Number.isFinite(maxTokens) ? maxTokens : Infinity;
  let overBudgetTokens = 0;

  for (const pack of selectionOrder) {
    const selection = selectVariant(pack, remainingTokens, renderCache);
    const decision = {
      id: pack.id,
      section: pack.section || null,
      priority: pack.priority,
      required: pack.required,
      status: selection.status,
      variant: selection.variant,
      chars: selection.chars,
      estimatedTokens: selection.estimatedTokens,
      order: pack.order,
      reason: selection.reason || null,
    };

    decisions.set(pack.id, decision);

    if (selection.status !== 'included') {
      continue;
    }

    selected.set(pack.id, {
      text: selection.text,
      chars: selection.chars,
      estimatedTokens: selection.estimatedTokens,
      variant: selection.variant,
    });

    if (!Number.isFinite(remainingTokens)) {
      continue;
    }

    if (selection.estimatedTokens > remainingTokens) {
      overBudgetTokens += selection.estimatedTokens - remainingTokens;
      remainingTokens = 0;
    } else {
      remainingTokens -= selection.estimatedTokens;
    }
  }

  const ordered = normalized.slice().sort(sortByOrder);
  let context = '';
  for (const pack of ordered) {
    const selectedPack = selected.get(pack.id);
    if (selectedPack) {
      context += selectedPack.text;
    }
  }

  const totalChars = context.length;
  const truncation = applyMaxCharsGuard({
    packs: ordered,
    selected,
    decisions,
    cache: renderCache,
    maxChars,
    totalChars,
  });

  if (truncation.applied) {
    context = '';
    for (const pack of ordered) {
      const selectedPack = selected.get(pack.id);
      if (selectedPack) {
        context += selectedPack.text;
      }
    }
  }

  const finalChars = context.length;
  const finalTokens = estimateTokensFromChars(finalChars);
  const packDecisions = ordered.map((pack) => {
    const decision = decisions.get(pack.id);
    return {
      id: decision.id,
      section: decision.section,
      priority: decision.priority,
      required: decision.required,
      status: decision.status,
      variant: decision.variant,
      chars: decision.chars,
      estimatedTokens: decision.estimatedTokens,
      order: decision.order,
      reason: decision.reason,
      truncated: decision.truncated || false,
    };
  });

  return {
    context,
    packDecisions,
    budget: {
      maxTokens,
      remainingTokens: Number.isFinite(remainingTokens) ? remainingTokens : null,
      overBudgetTokens,
      finalTokens,
    },
    truncation: {
      maxContextChars: {
        applied: truncation.applied,
        beforeChars: truncation.beforeChars,
        afterChars: truncation.afterChars,
      },
    },
  };
}

module.exports = {
  buildContextPacks,
};
