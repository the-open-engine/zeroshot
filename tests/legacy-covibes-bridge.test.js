const assert = require('assert');
const EventEmitter = require('events');

const bridge = require('../legacy/covibes-zeroshot-bridge/cli/index.js');

describe('Covibes Zeroshot legacy bridge', function () {
  it('derives the legacy package npm prefix from a global package root', function () {
    const prefix = bridge.deriveInstallPrefixFromPackageRoot(
      '/opt/homebrew/lib/node_modules/@covibes/zeroshot'
    );

    assert.strictEqual(prefix, '/opt/homebrew');
  });

  it('builds the forced install command that lets the new package take over the bin', function () {
    assert.deepStrictEqual(bridge.buildInstallArgs('/tmp/zeroshot-prefix'), [
      'install',
      '-g',
      '--prefix',
      '/tmp/zeroshot-prefix',
      '--force',
      '@the-open-engine/zeroshot@latest',
    ]);
  });

  it('runs update through npm with the resolved prefix', async function () {
    let spawnCommand = null;
    let spawnArgs = null;
    let spawnOptions = null;

    const spawn = (command, args, options) => {
      spawnCommand = command;
      spawnArgs = args;
      spawnOptions = options;

      const proc = new EventEmitter();
      process.nextTick(() => proc.emit('close', 0));
      return proc;
    };

    const success = await bridge.runUpdate({
      installPrefix: '/tmp/zeroshot-prefix',
      npmCommand: '/tmp/npm-for-test',
      spawn,
    });

    assert.strictEqual(success, true);
    assert.strictEqual(spawnCommand, '/tmp/npm-for-test');
    assert.deepStrictEqual(spawnArgs, [
      'install',
      '-g',
      '--prefix',
      '/tmp/zeroshot-prefix',
      '--force',
      '@the-open-engine/zeroshot@latest',
    ]);
    assert.deepStrictEqual(spawnOptions, {
      stdio: 'inherit',
      shell: false,
    });
  });
});
