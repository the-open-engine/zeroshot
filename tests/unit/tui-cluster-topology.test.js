const assert = require('assert');
const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');
const { loadTuiModule } = require('../helpers/load-tui');

const buildOutput = path.join(
  __dirname,
  '..',
  '..',
  'lib',
  'tui',
  'services',
  'cluster-topology.js'
);

function ensureTuiBuild() {
  if (!fs.existsSync(buildOutput)) {
    execSync('npm run build:tui', { stdio: 'inherit' });
  }
}

ensureTuiBuild();

let buildTopologyModel;

before(async function () {
  ({ buildTopologyModel } = await loadTuiModule('lib/tui/services/cluster-topology.js'));
});

function edgeKey(edge) {
  return `${edge.from}=>${edge.to}::${edge.topic}`;
}

describe('TUI cluster topology', function () {
  it('builds agents and edges including dynamic topics', function () {
    const config = {
      agents: [
        {
          id: 'planner',
          role: 'planning',
          triggers: [{ topic: 'ISSUE_OPENED', action: 'execute_task' }],
          hooks: {
            onComplete: {
              config: { topic: 'PLAN_READY' },
              logic: {
                engine: 'javascript',
                script: "if (ok) { return { topic: 'PLANNER_PROGRESS' }; }",
              },
            },
          },
        },
        {
          id: 'worker',
          role: 'implementation',
          triggers: [{ topic: 'PLAN_READY', action: 'execute_task' }],
          hooks: {
            onComplete: {
              config: { topic: 'IMPLEMENTATION_READY' },
              transform: {
                engine: 'javascript',
                script: "return { topic: 'WORKER_PROGRESS' };",
              },
            },
          },
        },
        {
          id: 'validator',
          role: 'validator',
          triggers: [{ topic: 'IMPLEMENTATION_READY', action: 'execute_task' }],
          hooks: {
            onComplete: {
              config: { topic: 'VALIDATION_RESULT' },
            },
          },
        },
        {
          id: 'newcomer',
          role: 'observer',
          triggers: [],
          hooks: {},
        },
      ],
    };

    const topology = buildTopologyModel(config);
    assert.deepStrictEqual(
      topology.agents.map((agent) => agent.id),
      ['planner', 'worker', 'validator', 'newcomer']
    );

    const edges = new Set(topology.edges.map(edgeKey));

    assert.ok(edges.has('system=>ISSUE_OPENED::ISSUE_OPENED'));
    assert.ok(edges.has('ISSUE_OPENED=>planner::ISSUE_OPENED'));
    assert.ok(edges.has('planner=>PLAN_READY::PLAN_READY'));
    assert.ok(edges.has('planner=>PLANNER_PROGRESS::PLANNER_PROGRESS'));
    assert.ok(edges.has('PLAN_READY=>worker::PLAN_READY'));
    assert.ok(edges.has('worker=>IMPLEMENTATION_READY::IMPLEMENTATION_READY'));
    assert.ok(edges.has('worker=>WORKER_PROGRESS::WORKER_PROGRESS'));
    assert.ok(edges.has('IMPLEMENTATION_READY=>validator::IMPLEMENTATION_READY'));
    assert.ok(edges.has('validator=>VALIDATION_RESULT::VALIDATION_RESULT'));
  });
});
