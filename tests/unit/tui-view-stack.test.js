const assert = require('assert');
const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');
const { loadTuiModule } = require('../helpers/load-tui');

const buildOutput = path.join(__dirname, '..', '..', 'lib', 'tui', 'view-stack.js');

function ensureTuiBuild() {
  if (!fs.existsSync(buildOutput)) {
    execSync('npm run build:tui', { stdio: 'inherit' });
  }
}

ensureTuiBuild();

let createViewStack;
let pushView;
let popView;
let activeView;
let DEFAULT_VIEW;

before(async function () {
  ({ createViewStack, pushView, popView, activeView, DEFAULT_VIEW } =
    await loadTuiModule('lib/tui/view-stack.js'));
});

describe('TUI view stack', function () {
  it('pushes and pops views', function () {
    let stack = createViewStack();
    stack = pushView(stack, 'monitor');
    stack = pushView(stack, 'cluster');
    assert.strictEqual(activeView(stack), 'cluster');

    stack = popView(stack);
    assert.strictEqual(activeView(stack), 'monitor');
  });

  it('does not pop below launcher', function () {
    const stack = createViewStack();
    const popped = popView(stack);
    assert.deepStrictEqual(popped, stack);
    assert.strictEqual(activeView(popped), DEFAULT_VIEW);
  });

  it('creates a stack with a custom initial view', function () {
    const stack = createViewStack('agent');
    assert.strictEqual(activeView(stack), 'agent');
  });
});
