const assert = require('assert');
const fs = require('fs');
const path = require('path');

describe('Full workflow fatal failure hooks', () => {
  const templatePath = path.join(
    __dirname,
    '..',
    '..',
    'cluster-templates',
    'base-templates',
    'full-workflow.json'
  );
  let template;

  before(() => {
    template = JSON.parse(fs.readFileSync(templatePath, 'utf8'));
  });

  it('planner publishes CLUSTER_FAILED on final task failure', () => {
    const planner = template.agents.find((agent) => agent.id === 'planner');
    assert.ok(planner, 'planner agent must exist');
    assert.ok(planner.hooks?.onError, 'planner must define onError hook');
    assert.strictEqual(planner.hooks.onError.action, 'publish_message');
    assert.strictEqual(planner.hooks.onError.config?.topic, 'CLUSTER_FAILED');
    assert.strictEqual(
      planner.hooks.onError.config?.content?.data?.reason,
      'planning_agent_failed'
    );
  });

  it('worker publishes CLUSTER_FAILED on final task failure', () => {
    const worker = template.agents.find((agent) => agent.id === 'worker');
    assert.ok(worker, 'worker agent must exist');
    assert.ok(worker.hooks?.onError, 'worker must define onError hook');
    assert.strictEqual(worker.hooks.onError.action, 'publish_message');
    assert.strictEqual(worker.hooks.onError.config?.topic, 'CLUSTER_FAILED');
    assert.strictEqual(
      worker.hooks.onError.config?.content?.data?.reason,
      'implementation_agent_failed'
    );
  });
});
