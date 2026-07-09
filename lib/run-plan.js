/**
 * The single canonical run-mode resolver.
 *
 * Every consumer (orchestrator, daemon startup, CLI label) derives isolation,
 * delivery, and autoMerge from THIS function and nowhere else. The raw
 * worktree/docker/pr/ship/autoMerge booleans are inputs; the frozen plan is the
 * one truth. Deriving autoMerge separately (the old `Boolean(ship)` scatter) is
 * what let `--pr` merge and let the run-mode label drift from behavior.
 */

function resolveIsolation(options) {
  if (options.docker) return 'docker';
  if (options.worktree || options.pr || options.ship) return 'worktree';
  return 'none';
}

function resolveDelivery(options) {
  // Explicit autoMerge (e.g. a future `--auto-merge` flag) is ship-equivalent:
  // "merge it" implies the ship delivery. This is the ONLY place that intent is
  // interpreted, so it can never be silently overwritten downstream.
  if (options.ship || options.autoMerge === true) return 'ship';
  if (options.pr) return 'pr';
  return 'none';
}

function resolveRunPlan(options = {}) {
  const isolation = resolveIsolation(options);
  const delivery = resolveDelivery(options);
  return Object.freeze({ isolation, delivery, autoMerge: delivery === 'ship' });
}

module.exports = { resolveRunPlan };
