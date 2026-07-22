/**
 * Plan checks (v1): deterministic, pre-execution checks over the planner's
 * OPTIONAL structured step skeleton (full-workflow planner jsonSchema
 * `steps`, forwarded onto PLAN_READY data). The prose `plan` string remains
 * the source of truth for humans; these checks treat the steps as data —
 * the "macroexpand" pass over a plan before the worker evals it.
 *
 * v1 is OBSERVATION ONLY: findings are published as PLAN_CHECK_RESULT
 * warning messages in the run ledger and never block, rewrite, or reroute
 * the plan. Gating belongs to a later version, informed by the warning
 * stream v1 produces.
 *
 * Rules:
 *  1. scope-containment — every mutating step (edit/create) with a target
 *     must be covered by the declared filesAffected (exact file match or a
 *     declared directory prefix). Catches silent scope drift at plan time.
 *  2. edit-requires-verify — every mutating step must be checkable: either
 *     it carries its own `verify` field or a LATER step (plans execute in
 *     array order) has kind 'verify'. The plan-tense analogue of LYO's
 *     "edit without verification" lesson.
 *  3. step-shape — ids present and unique, kind within the closed
 *     vocabulary, dependsOn references resolvable. Catches degenerate
 *     structured output (the planner-competence measurement for v0).
 */

const PLAN_CHECK_TOPIC = 'PLAN_CHECK_RESULT';

const STEP_KINDS = new Set(['inspect', 'edit', 'create', 'run', 'verify']);
const MUTATING_KINDS = new Set(['edit', 'create']);

function normalizePath(value) {
  return String(value || '')
    .replace(/\\/g, '/')
    .replace(/^\.\//, '')
    .replace(/\/{2,}/g, '/')
    .replace(/\/+$/, '');
}

function finding(rule, stepId, message) {
  return { rule, severity: 'warning', step_id: stepId ?? null, message };
}

// Rule 1: mutating step targets must stay inside the declared filesAffected.
function checkScopeContainment(steps, filesAffected) {
  if (!Array.isArray(filesAffected) || filesAffected.length === 0) {
    return []; // nothing declared to check against
  }
  const declared = filesAffected.map(normalizePath).filter(Boolean);
  const findings = [];
  for (const step of steps) {
    if (!MUTATING_KINDS.has(step.kind) || !step.target) continue;
    const target = normalizePath(step.target);
    const covered = declared.some((entry) => target === entry || target.startsWith(`${entry}/`));
    if (!covered) {
      findings.push(
        finding(
          'scope-containment',
          step.id,
          `step ${step.id} (${step.kind}) targets '${step.target}', which is outside the declared filesAffected`
        )
      );
    }
  }
  return findings;
}

// Rule 2: every mutating step needs a way to be checked after it runs.
function checkEditRequiresVerify(steps) {
  const findings = [];
  steps.forEach((step, index) => {
    if (!MUTATING_KINDS.has(step.kind)) return;
    if (typeof step.verify === 'string' && step.verify.trim().length > 0) return;
    const hasLaterVerify = steps.slice(index + 1).some((later) => later.kind === 'verify');
    if (!hasLaterVerify) {
      findings.push(
        finding(
          'edit-requires-verify',
          step.id,
          `step ${step.id} (${step.kind}) has no verify field and no later 'verify' step checks it`
        )
      );
    }
  });
  return findings;
}

// Rule 3: ids present/unique, kind in vocabulary, dependsOn resolvable.
function checkStepShape(steps) {
  const findings = [];
  const seenIds = new Set();
  for (const step of steps) {
    if (!step.id) {
      findings.push(finding('step-shape', null, `a step is missing its id (kind: ${step.kind})`));
    } else if (seenIds.has(step.id)) {
      findings.push(finding('step-shape', step.id, `duplicate step id '${step.id}'`));
    } else {
      seenIds.add(step.id);
    }
    if (!STEP_KINDS.has(step.kind)) {
      findings.push(
        finding('step-shape', step.id, `step ${step.id} has unknown kind '${step.kind}'`)
      );
    }
  }
  for (const step of steps) {
    if (!Array.isArray(step.dependsOn)) continue;
    for (const dependency of step.dependsOn) {
      if (!seenIds.has(dependency)) {
        findings.push(
          finding(
            'step-shape',
            step.id,
            `step ${step.id} dependsOn unknown step id '${dependency}'`
          )
        );
      }
    }
  }
  return findings;
}

/**
 * Run all rules over a structured plan. Returns { findings } — an empty
 * array means the plan passed every check.
 */
function checkPlan({ steps, filesAffected }) {
  const findings = [
    ...checkStepShape(steps),
    ...checkScopeContainment(steps, filesAffected),
    ...checkEditRequiresVerify(steps),
  ];
  return { findings };
}

/**
 * Subscribe to PLAN_READY and log findings as PLAN_CHECK_RESULT warnings.
 * Prose-only plans (no steps array — the v0 skeleton is optional) are
 * skipped silently. Disabled via cluster.config.planChecks.enabled === false.
 * Blast-radius containment mirrors the LYO observer: any failure degrades
 * to a console warning; plan flow is never interrupted.
 */
function attachPlanChecker({ messageBus, cluster }) {
  if (!messageBus) {
    throw new Error('attachPlanChecker: messageBus is required');
  }
  if (!cluster?.id) {
    throw new Error('attachPlanChecker: cluster.id is required');
  }
  if (cluster?.config?.planChecks?.enabled === false) {
    return () => {};
  }

  const unsubscribe = messageBus.subscribe((message) => {
    if (message.cluster_id !== cluster.id || message.topic !== 'PLAN_READY') {
      return;
    }
    try {
      const data = message.content?.data ?? {};
      const steps = Array.isArray(data.steps) ? data.steps : null;
      if (!steps || steps.length === 0) {
        return; // prose-only plan: nothing machine-checkable yet
      }
      const { findings } = checkPlan({ steps, filesAffected: data.filesAffected });
      if (findings.length === 0) {
        return;
      }
      messageBus.publish({
        cluster_id: cluster.id,
        topic: PLAN_CHECK_TOPIC,
        sender: 'plan-checker',
        content: {
          text: `Plan check: ${findings.length} warning(s) for plan published in ${message.id}.`,
          data: {
            plan_message_id: message.id,
            severity: 'warning',
            findings,
          },
        },
        metadata: {
          source: 'plan_checks',
        },
      });
    } catch (error) {
      console.warn('[plan-checks] check failed, skipping:', error.message);
    }
  });

  return unsubscribe;
}

module.exports = {
  PLAN_CHECK_TOPIC,
  STEP_KINDS,
  checkPlan,
  attachPlanChecker,
};
