const assert = require('assert');

const { resolveEffectiveRunPlan, startClusterFromText } = require('../../lib/start-cluster');

function captureStartOptions(settings) {
  const config = { agents: [] };
  const orchestrator = {
    start(_config, _input, startOptions) {
      return startOptions;
    },
  };
  return startClusterFromText({
    orchestrator,
    config,
    clusterId: 'c1',
    text: 'hello',
    options: {},
    settings,
  });
}

describe('resolveEffectiveRunPlan() settings.defaultDelivery (issue #606)', function () {
  it('folds merged defaultDelivery=ship into delivery + autoMerge', function () {
    const plan = resolveEffectiveRunPlan({ ship: true }, {});
    assert.strictEqual(plan.delivery, 'ship');
    assert.strictEqual(plan.autoMerge, true);
    assert.strictEqual(plan.isolation, 'worktree');
  });

  it('folds merged defaultDelivery=pr into delivery without autoMerge', function () {
    const plan = resolveEffectiveRunPlan({ pr: true }, {});
    assert.strictEqual(plan.delivery, 'pr');
    assert.strictEqual(plan.autoMerge, false);
    assert.strictEqual(plan.isolation, 'worktree');
  });

  it('defaults to delivery=none when settings.defaultDelivery is unset', function () {
    const plan = resolveEffectiveRunPlan({}, {});
    assert.strictEqual(plan.delivery, 'none');
    assert.strictEqual(plan.autoMerge, false);
  });

  it('a CLI --pr flag still wins when settings.defaultDelivery=none', function () {
    const plan = resolveEffectiveRunPlan({ pr: true }, { defaultDelivery: 'none' });
    assert.strictEqual(plan.delivery, 'pr');
  });

  it('startClusterFromText folds settings.defaultDelivery into autoPr/autoMerge', function () {
    const result = captureStartOptions({ defaultDelivery: 'ship' });
    assert.strictEqual(result.autoPr, true);
    assert.strictEqual(result.autoMerge, true);
    assert.strictEqual(result.worktree, true);
  });
});
