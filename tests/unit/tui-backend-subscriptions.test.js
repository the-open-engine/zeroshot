const assert = require('assert');
const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');

const PROJECT_ROOT = path.resolve(__dirname, '../..');
const SOURCE_PATH = path.join(PROJECT_ROOT, 'src', 'tui-backend', 'subscriptions', 'index.ts');
const BUILD_PATH = path.join(PROJECT_ROOT, 'lib', 'tui-backend', 'subscriptions', 'index.js');

function isBuildStale(sourcePath, buildPath) {
  if (!fs.existsSync(buildPath)) {
    return true;
  }
  if (!fs.existsSync(sourcePath)) {
    return false;
  }
  return fs.statSync(sourcePath).mtimeMs > fs.statSync(buildPath).mtimeMs;
}

if (isBuildStale(SOURCE_PATH, BUILD_PATH)) {
  execSync('npm run build:tui-backend', { cwd: PROJECT_ROOT, stdio: 'inherit' });
}

const { createSubscriptionRegistry } = require('../../lib/tui-backend/subscriptions');

describe('tui-backend subscription registry', function () {
  it('closes subscriptions exactly once', function () {
    const registry = createSubscriptionRegistry();
    let closes = 0;

    const id = registry.add('logs', () => {
      closes += 1;
    });

    const first = registry.unsubscribe(id);
    assert.deepStrictEqual(first, { removed: true });
    const second = registry.unsubscribe(id);
    assert.deepStrictEqual(second, { removed: false });

    assert.strictEqual(closes, 1);
    assert.strictEqual(registry.size(), 0);
  });

  it('closeAll closes remaining subscriptions and clears registry', function () {
    const registry = createSubscriptionRegistry();
    let closes = 0;

    registry.add('logs', () => {
      closes += 1;
    });
    registry.add('timeline', () => {
      closes += 1;
    });

    const count = registry.closeAll();
    assert.strictEqual(count, 2);
    assert.strictEqual(closes, 2);
    assert.strictEqual(registry.size(), 0);

    const again = registry.closeAll();
    assert.strictEqual(again, 0);
    assert.strictEqual(closes, 2);
  });
});
