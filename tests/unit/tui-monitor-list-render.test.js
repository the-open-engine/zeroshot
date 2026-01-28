const assert = require('assert');
const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');

const buildOutput = path.join(__dirname, '..', '..', 'lib', 'tui', 'views', 'MonitorView.js');

function ensureTuiBuild() {
  if (!fs.existsSync(buildOutput)) {
    execSync('npm run build:tui', { stdio: 'inherit' });
  }
}

ensureTuiBuild();

const {
  buildMonitorRows,
  computeMonitorColumnWidths,
  formatMonitorRowLine,
} = require('../../lib/tui/views/MonitorView');

describe('TUI monitor list rendering', function () {
  it('builds rows and formats list lines', function () {
    const now = 1700000000000;
    const longId = 'cluster-identifier-with-very-long-id';
    const longCwd = `/var/tmp/${'x'.repeat(90)}`;

    const clusters = [
      {
        id: 'short',
        state: 'running',
        createdAt: now - 45 * 1000,
        agentCount: 2,
        messageCount: 10,
        cwd: '/tmp/alpha',
      },
      {
        id: longId,
        state: 'failed',
        createdAt: now - 3 * 60 * 60 * 1000,
        agentCount: 1,
        messageCount: 0,
        cwd: longCwd,
      },
    ];

    const metricsById = {
      short: { id: 'short', supported: true, cpuPercent: 12.34, memoryMB: 512.3 },
      [longId]: { id: longId, supported: true, cpuPercent: null, memoryMB: null },
    };

    const rows = buildMonitorRows(clusters, metricsById, now);
    assert.strictEqual(rows[0].age, '45s');
    assert.strictEqual(rows[0].cpuDisplay, '12.3%');
    assert.strictEqual(rows[0].memoryDisplay, '512MB');
    assert.strictEqual(rows[1].age, '3h');
    assert.strictEqual(rows[1].cpuDisplay, '-');
    assert.strictEqual(rows[1].memoryDisplay, '-');

    const widths = computeMonitorColumnWidths(rows);
    assert.deepStrictEqual(widths, {
      idWidth: 24,
      statusWidth: 7,
      ageWidth: 3,
      cpuWidth: 5,
      memWidth: 5,
    });

    const line = formatMonitorRowLine(rows[1], widths);
    const expectedSuffix = `${longCwd.slice(0, 77)}...`;
    assert.ok(line.startsWith(longId.slice(0, widths.idWidth)));
    assert.ok(line.includes('failed'));
    assert.ok(line.endsWith(expectedSuffix));
  });
});
