const EVENT_COPY = {
  IMPLEMENTATION_READY: 'Implementation ready',
  PR_CREATED: 'Pull request created',
};

function formatMergeStatus(merged) {
  if (merged === true || merged === 'true') return 'merged';
  if (merged === false || merged === 'false') return 'auto-merge pending approval';
  return null;
}

module.exports = { EVENT_COPY, formatMergeStatus };
