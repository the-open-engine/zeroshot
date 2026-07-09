const { resolveRunPlan } = require('./run-plan');

// The run-mode label is a VIEW of the canonical plan, never an independent
// cascade. Deriving it from the plan is what keeps the user-facing label and the
// actual isolation/delivery/autoMerge behavior from drifting apart. Callers that
// already hold a plan (e.g. the effective plan with env/settings folded in) use
// runModeFromPlan directly so the label reflects the SAME plan as behavior.
function runModeFromPlan({ isolation, delivery }) {
  const dockerSuffix = isolation === 'docker' ? '+docker' : '';
  if (delivery === 'ship') return `ship${dockerSuffix}`;
  if (delivery === 'pr') return `pr${dockerSuffix}`;
  if (isolation === 'docker') return 'docker';
  if (isolation === 'worktree') return 'worktree';
  return null;
}

function resolveRunMode(options) {
  return runModeFromPlan(resolveRunPlan(options));
}

const RUN_MODE_LABELS = {
  ship: 'ship (worktree + PR + auto-merge)',
  'ship+docker': 'ship (docker + PR + auto-merge)',
  pr: 'pr (worktree + PR)',
  'pr+docker': 'pr (docker + PR)',
  docker: 'docker (isolated container)',
  worktree: 'worktree (isolated branch)',
};

function describeRunMode(mode) {
  return RUN_MODE_LABELS[mode] || 'local (no isolation)';
}

module.exports = { resolveRunMode, runModeFromPlan, describeRunMode };
