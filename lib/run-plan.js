function resolveIsolation(options) {
  if (options.docker) return 'docker';
  if (options.worktree || options.pr || options.ship) return 'worktree';
  return 'none';
}

function resolveDelivery(options) {
  if (options.ship) return 'ship';
  if (options.pr) return 'pr';
  return 'none';
}

function resolveRunPlan(options = {}) {
  const isolation = resolveIsolation(options);
  const delivery = resolveDelivery(options);
  return Object.freeze({ isolation, delivery, autoMerge: delivery === 'ship' });
}

module.exports = { resolveRunPlan };
