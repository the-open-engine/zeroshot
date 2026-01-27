const SNAPSHOT_VERSION = 1;

const LIMITS = {
  errors: 5,
  criteriaResults: 10,
  acceptanceCriteria: 10,
  filesAffected: 20,
  blockers: 5,
  nextSteps: 10,
  rootCauses: 5,
};

const TEXT_LIMITS = {
  task: 2000,
  plan: 2000,
  fixPlan: 1200,
  summary: 300,
  listItem: 200,
};

function toTimestamp(message) {
  if (message && Number.isFinite(message.timestamp)) {
    return message.timestamp;
  }
  return Date.now();
}

function normalizeText(value, maxLength, singleLine = false) {
  if (value === undefined || value === null) return undefined;
  let text = String(value);
  if (singleLine) {
    text = text.replace(/\s+/g, ' ').trim();
  } else {
    text = text.trim();
  }
  if (!text) return undefined;
  if (maxLength && text.length > maxLength) {
    return `${text.slice(0, maxLength - 3)}...`;
  }
  return text;
}

function normalizeStringList(list, maxItems) {
  if (!Array.isArray(list)) return undefined;
  const normalized = list
    .map((item) => normalizeText(item, TEXT_LIMITS.listItem, true))
    .filter(Boolean);
  if (normalized.length === 0) return undefined;
  if (maxItems && normalized.length > maxItems) {
    return normalized.slice(-maxItems);
  }
  return normalized;
}

function normalizeAcceptanceCriteria(criteria) {
  if (!Array.isArray(criteria)) return undefined;
  const normalized = criteria
    .map((item) => {
      if (typeof item === 'string') {
        return normalizeText(item, TEXT_LIMITS.listItem, true);
      }
      if (!item || typeof item !== 'object') return undefined;
      const id = item.id ? String(item.id) : '';
      const priority = item.priority ? ` (${item.priority})` : '';
      const criterion = item.criterion || item.text || item.summary || '';
      const label = id ? `${id}${priority}: ` : '';
      const merged = `${label}${criterion}`.trim();
      if (!merged) return undefined;
      return normalizeText(merged, TEXT_LIMITS.listItem, true);
    })
    .filter(Boolean);
  if (normalized.length === 0) return undefined;
  if (normalized.length > LIMITS.acceptanceCriteria) {
    return normalized.slice(-LIMITS.acceptanceCriteria);
  }
  return normalized;
}

function normalizeCriteriaEvidence(evidence) {
  if (!evidence || typeof evidence !== 'object') return undefined;
  const normalized = {};
  if (evidence.command) {
    const command = normalizeText(evidence.command, TEXT_LIMITS.listItem, true);
    if (command) normalized.command = command;
  }
  if (Number.isFinite(evidence.exitCode)) {
    normalized.exitCode = evidence.exitCode;
  }
  return Object.keys(normalized).length > 0 ? normalized : undefined;
}

function normalizeCriteriaResult(item) {
  if (!item || typeof item !== 'object') return undefined;
  const entry = {};
  if (item.id) entry.id = String(item.id);
  if (item.status) entry.status = String(item.status);
  if (item.reason) {
    const reason = normalizeText(item.reason, TEXT_LIMITS.listItem, true);
    if (reason) entry.reason = reason;
  }
  const evidence = normalizeCriteriaEvidence(item.evidence);
  if (evidence) entry.evidence = evidence;
  return Object.keys(entry).length > 0 ? entry : undefined;
}

function normalizeCriteriaResults(results) {
  if (!Array.isArray(results)) return undefined;
  const normalized = results.map(normalizeCriteriaResult).filter(Boolean);
  if (normalized.length === 0) return undefined;
  if (normalized.length > LIMITS.criteriaResults) {
    return normalized.slice(-LIMITS.criteriaResults);
  }
  return normalized;
}

function normalizeErrors(data) {
  if (!data || typeof data !== 'object') return undefined;
  if (Array.isArray(data.errors)) {
    return normalizeStringList(data.errors, LIMITS.errors);
  }
  if (Array.isArray(data.issues)) {
    const mapped = data.issues.map((issue) => {
      if (typeof issue === 'string') return issue;
      if (!issue || typeof issue !== 'object') return undefined;
      return issue.bug || issue.message || issue.error || issue.summary || undefined;
    });
    return normalizeStringList(mapped, LIMITS.errors);
  }
  return undefined;
}

function normalizeRootCauses(rootCauses) {
  if (!Array.isArray(rootCauses)) return undefined;
  const normalized = rootCauses
    .map((cause) => {
      if (typeof cause === 'string') {
        return normalizeText(cause, TEXT_LIMITS.listItem, true);
      }
      if (!cause || typeof cause !== 'object') return undefined;
      return normalizeText(
        cause.cause || cause.summary || cause.description,
        TEXT_LIMITS.listItem,
        true
      );
    })
    .filter(Boolean);
  if (normalized.length === 0) return undefined;
  if (normalized.length > LIMITS.rootCauses) {
    return normalized.slice(-LIMITS.rootCauses);
  }
  return normalized;
}

function normalizeFilesAffected(filesAffected) {
  return normalizeStringList(filesAffected, LIMITS.filesAffected);
}

function normalizeBoolean(value) {
  if (typeof value === 'boolean') return value;
  if (value === 'true') return true;
  if (value === 'false') return false;
  return undefined;
}

function normalizeProgressStatus(data) {
  if (!data || typeof data !== 'object') return undefined;
  if (data.completionStatus && typeof data.completionStatus === 'object') {
    return data.completionStatus;
  }
  const hasProgressFields =
    Object.prototype.hasOwnProperty.call(data, 'canValidate') ||
    Object.prototype.hasOwnProperty.call(data, 'percentComplete');
  return hasProgressFields ? data : undefined;
}

function buildBaseState(state, message) {
  return {
    version: SNAPSHOT_VERSION,
    updatedAt: toTimestamp(message),
    clusterId: message?.cluster_id || state?.clusterId || null,
    sourceMessageId: message?.id || state?.sourceMessageId || null,
    task: state?.task,
    plan: state?.plan,
    progress: state?.progress,
    validation: state?.validation,
    debug: state?.debug,
  };
}

function pruneEmpty(value) {
  if (Array.isArray(value)) {
    const next = value.map(pruneEmpty).filter((item) => item !== undefined);
    return next.length > 0 ? next : undefined;
  }
  if (value && typeof value === 'object') {
    const next = {};
    for (const [key, entry] of Object.entries(value)) {
      const pruned = pruneEmpty(entry);
      if (pruned !== undefined) {
        next[key] = pruned;
      }
    }
    return Object.keys(next).length > 0 ? next : undefined;
  }
  if (value === undefined || value === null) return undefined;
  return value;
}

function finalizeState(state) {
  const meta = {
    version: state.version ?? SNAPSHOT_VERSION,
    updatedAt: state.updatedAt ?? Date.now(),
    clusterId: state.clusterId ?? null,
    sourceMessageId: state.sourceMessageId ?? null,
  };
  const sections = pruneEmpty({
    task: state.task,
    plan: state.plan,
    progress: state.progress,
    validation: state.validation,
    debug: state.debug,
  });
  return {
    ...meta,
    ...(sections || {}),
  };
}

function initStateFromIssue(issueMessage) {
  const content = issueMessage?.content || {};
  const data = content.data || {};
  const task = {
    raw: normalizeText(content.text, TEXT_LIMITS.task),
    title: normalizeText(data.title, TEXT_LIMITS.summary, true),
    issueNumber: data.issue_number ?? data.issueNumber,
    source: issueMessage?.metadata?.source,
  };
  const base = buildBaseState(null, issueMessage);
  base.task = pruneEmpty(task);
  return finalizeState(base);
}

function applyIssueOpened(state, message) {
  const base = buildBaseState(state, message);
  const content = message?.content || {};
  const data = content.data || {};
  const task = {
    raw: normalizeText(content.text, TEXT_LIMITS.task),
    title: normalizeText(data.title, TEXT_LIMITS.summary, true),
    issueNumber: data.issue_number ?? data.issueNumber,
    source: message?.metadata?.source,
  };
  base.task = pruneEmpty(task);
  return finalizeState(base);
}

function applyPlanReady(state, message) {
  const base = buildBaseState(state, message);
  const content = message?.content || {};
  const data = content.data || {};
  const plan = {
    text: normalizeText(content.text, TEXT_LIMITS.plan),
    summary: normalizeText(data.summary, TEXT_LIMITS.summary, true),
    acceptanceCriteria: normalizeAcceptanceCriteria(data.acceptanceCriteria),
    filesAffected: normalizeFilesAffected(data.filesAffected),
    updatedAt: toTimestamp(message),
  };
  base.plan = pruneEmpty(plan);
  return finalizeState(base);
}

function applyWorkerProgress(state, message) {
  const base = buildBaseState(state, message);
  const content = message?.content || {};
  const status = normalizeProgressStatus(content.data || {});
  if (!status) {
    return finalizeState(base);
  }
  const progress = {
    canValidate: normalizeBoolean(status.canValidate),
    percentComplete: Number.isFinite(status.percentComplete) ? status.percentComplete : undefined,
    blockers: normalizeStringList(status.blockers, LIMITS.blockers),
    nextSteps: normalizeStringList(status.nextSteps, LIMITS.nextSteps),
    lastSummary: normalizeText(content.text || status.summary, TEXT_LIMITS.summary, true),
    updatedAt: toTimestamp(message),
  };
  base.progress = pruneEmpty(progress);
  return finalizeState(base);
}

function applyImplementationReady(state, message) {
  return applyWorkerProgress(state, message);
}

function applyValidationResult(state, message) {
  const base = buildBaseState(state, message);
  const content = message?.content || {};
  const data = content.data || {};
  const validation = {
    approved: normalizeBoolean(data.approved),
    errors: normalizeErrors(data),
    criteriaResults: normalizeCriteriaResults(data.criteriaResults),
    updatedAt: toTimestamp(message),
  };
  base.validation = pruneEmpty(validation);
  return finalizeState(base);
}

function applyInvestigationComplete(state, message) {
  const base = buildBaseState(state, message);
  const content = message?.content || {};
  const data = content.data || {};
  const debug = {
    fixPlan: normalizeText(content.text, TEXT_LIMITS.fixPlan),
    successCriteria: normalizeText(data.successCriteria, TEXT_LIMITS.summary, true),
    rootCauses: normalizeRootCauses(data.rootCauses),
    updatedAt: toTimestamp(message),
  };
  base.debug = pruneEmpty(debug);
  return finalizeState(base);
}

function buildTaskSummary(state) {
  const taskTitle = normalizeText(state.task?.title || state.task?.raw, TEXT_LIMITS.summary, true);
  return taskTitle ? `Task: ${taskTitle}` : undefined;
}

function buildPlanSummary(state) {
  const planSummary = normalizeText(
    state.plan?.summary || state.plan?.text,
    TEXT_LIMITS.summary,
    true
  );
  return planSummary ? `Plan: ${planSummary}` : undefined;
}

function buildProgressSummary(state) {
  if (!state.progress) return undefined;
  const parts = [];
  if (Number.isFinite(state.progress.percentComplete)) {
    parts.push(`${state.progress.percentComplete}%`);
  }
  if (typeof state.progress.canValidate === 'boolean') {
    parts.push(`canValidate=${state.progress.canValidate}`);
  }
  const nextStepText = normalizeText(state.progress.nextSteps?.[0], TEXT_LIMITS.listItem, true);
  if (nextStepText) {
    parts.push(`next: ${nextStepText}`);
  }
  return parts.length > 0 ? `Progress: ${parts.join(' | ')}` : undefined;
}

function resolveValidationStatus(approved) {
  if (approved === true) return 'approved';
  if (approved === false) return 'rejected';
  return 'pending';
}

function buildValidationSummary(state) {
  if (!state.validation) return undefined;
  const status = resolveValidationStatus(state.validation.approved);
  const errorCount = state.validation.errors?.length || 0;
  return `Validation: ${status}${errorCount ? ` (${errorCount} errors)` : ''}`;
}

function buildDebugSummary(state) {
  const debugSummary = normalizeText(
    state.debug?.fixPlan || state.debug?.successCriteria,
    TEXT_LIMITS.summary,
    true
  );
  return debugSummary ? `Debug: ${debugSummary}` : undefined;
}

function renderStateSummary(state) {
  if (!state || typeof state !== 'object') return '';
  const lines = [
    buildTaskSummary(state),
    buildPlanSummary(state),
    buildProgressSummary(state),
    buildValidationSummary(state),
    buildDebugSummary(state),
  ].filter(Boolean);

  return lines.join('\n');
}

module.exports = {
  SNAPSHOT_VERSION,
  initStateFromIssue,
  applyIssueOpened,
  applyPlanReady,
  applyWorkerProgress,
  applyImplementationReady,
  applyValidationResult,
  applyInvestigationComplete,
  renderStateSummary,
};
