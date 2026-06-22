const assert = require('assert');
const sinon = require('sinon');
const {
  commandExists,
  getCommandPath,
  getHelpOutput,
  getVersionOutput,
  commandLookupCommand,
} = require('../../lib/provider-detection');

describe('Provider CLI detection', () => {
  it('detects commands by absolute path', () => {
    const nodePath = process.execPath;
    assert.strictEqual(commandExists(nodePath), true);
    assert.strictEqual(getCommandPath(nodePath), nodePath);
  });

  it('returns false for missing commands', () => {
    assert.strictEqual(commandExists('/nonexistent/cli'), false);
    assert.strictEqual(getCommandPath('/nonexistent/cli'), null);
  });

  it('returns help and version output when available', () => {
    const nodePath = process.execPath;
    const help = getHelpOutput(nodePath);
    const version = getVersionOutput(nodePath);

    assert.ok(typeof help === 'string');
    assert.ok(typeof version === 'string');
    assert.ok(version.length > 0);
  });
});

describe('commandLookupCommand', () => {
  let platformStub;

  afterEach(() => {
    if (platformStub) platformStub.restore();
  });

  it('uses where on Windows', () => {
    platformStub = sinon.stub(process, 'platform').value('win32');
    assert.strictEqual(commandLookupCommand('claude'), 'where claude');
  });

  it('uses command -v on non-Windows platforms', () => {
    platformStub = sinon.stub(process, 'platform').value('linux');
    assert.strictEqual(commandLookupCommand('claude'), 'command -v claude');
  });
});
