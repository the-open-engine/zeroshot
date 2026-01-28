const assert = require('assert');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { execSync } = require('child_process');

const buildOutput = path.join(
  __dirname,
  '..',
  '..',
  'lib',
  'tui',
  'services',
  'cluster-timeline.js'
);

function ensureTuiBuild() {
  if (!fs.existsSync(buildOutput)) {
    execSync('npm run build:tui', { stdio: 'inherit' });
  }
}

ensureTuiBuild();

const { createClusterTimelineStream } = require('../../lib/tui/services/cluster-timeline');
const Ledger = require('../../src/ledger');

describe('TUI cluster timeline service', function () {
  const originalHome = process.env.HOME;
  let tempHome;

  beforeEach(function () {
    tempHome = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-tui-timeline-'));
    process.env.HOME = tempHome;
  });

  afterEach(function () {
    process.env.HOME = originalHome;
    fs.rmSync(tempHome, { recursive: true, force: true });
  });

  it('streams workflow timeline events in order with labels', async function () {
    const clusterId = 'cluster-timeline-1';
    const zeroshotDir = path.join(tempHome, '.zeroshot');
    fs.mkdirSync(zeroshotDir, { recursive: true });

    const dbPath = path.join(zeroshotDir, `${clusterId}.db`);
    const writer = new Ledger(dbPath);

    writer.append({
      cluster_id: clusterId,
      topic: 'ISSUE_OPENED',
      sender: 'conductor',
      content: { data: { title: 'Test issue' } },
    });
    writer.append({
      cluster_id: clusterId,
      topic: 'AGENT_OUTPUT',
      sender: 'worker',
      content: { text: 'noise' },
    });
    writer.append({
      cluster_id: clusterId,
      topic: 'PLAN_READY',
      sender: 'planner',
      content: { data: { summary: 'Plan' } },
    });
    writer.append({
      cluster_id: clusterId,
      topic: 'IMPLEMENTATION_READY',
      sender: 'worker',
      content: { data: { summary: 'Done' } },
    });
    writer.append({
      cluster_id: clusterId,
      topic: 'VALIDATION_RESULT',
      sender: 'validator',
      content: { data: { approved: true } },
    });
    writer.close();

    const seen = [];

    const stream = createClusterTimelineStream({
      clusterId,
      pollIntervalMs: 25,
      onEvents: (events) => {
        seen.push(...events);
      },
    });

    stream.start();

    await new Promise((resolve) => setTimeout(resolve, 80));

    stream.close();

    const unique = new Map();
    for (const event of seen) {
      unique.set(event.id, event);
    }
    const ordered = Array.from(unique.values()).sort((a, b) => a.timestamp - b.timestamp);

    const topics = ordered.map((event) => event.topic);
    assert.deepStrictEqual(topics, [
      'ISSUE_OPENED',
      'PLAN_READY',
      'IMPLEMENTATION_READY',
      'VALIDATION_RESULT',
    ]);

    assert.ok(!topics.includes('AGENT_OUTPUT'));

    const validation = ordered.find((event) => event.topic === 'VALIDATION_RESULT');
    assert.ok(validation);
    assert.ok(validation.label.includes('Validation approved'));
  });
});
