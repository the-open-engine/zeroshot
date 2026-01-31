const assert = require('assert');
const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');
const { loadTuiModule } = require('../helpers/load-tui');

const buildOutput = path.join(__dirname, '..', '..', 'lib', 'tui', 'app.js');

function ensureTuiBuild() {
  if (!fs.existsSync(buildOutput)) {
    execSync('npm run build:tui', { stdio: 'inherit' });
  }
}

ensureTuiBuild();

let handleAppInput;
let activeView;
let createViewStack;
let popView;
let pushView;

before(async function () {
  ({ handleAppInput } = await loadTuiModule('lib/tui/app.js'));
  ({ activeView, createViewStack, popView, pushView } =
    await loadTuiModule('lib/tui/view-stack.js'));
});

describe('TUI Esc-back handling', function () {
  it('pops the view stack on escape', function () {
    let stack = createViewStack();
    stack = pushView(stack, 'monitor');

    let exitCalls = 0;
    let submitCalls = 0;
    let deleteCalls = 0;
    let appendCalls = 0;

    handleAppInput(
      '',
      { escape: true },
      {
        exit: () => {
          exitCalls += 1;
        },
        popView: () => {
          stack = popView(stack);
        },
        submit: () => {
          submitCalls += 1;
        },
        deleteChar: () => {
          deleteCalls += 1;
        },
        appendText: () => {
          appendCalls += 1;
        },
      }
    );

    assert.strictEqual(activeView(stack), 'launcher');
    assert.strictEqual(exitCalls, 0);
    assert.strictEqual(submitCalls, 0);
    assert.strictEqual(deleteCalls, 0);
    assert.strictEqual(appendCalls, 0);
  });
});
