function resolveRunMode(options) {
  if (options.ship) return options.docker ? 'ship+docker' : 'ship';
  if (options.pr) return options.docker ? 'pr+docker' : 'pr';
  if (options.docker) return 'docker';
  if (options.worktree) return 'worktree';
  return null;
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

module.exports = { resolveRunMode, describeRunMode };
