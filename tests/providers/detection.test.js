const assert = require('assert');
const fs = require('fs');
const os = require('os');
const path = require('path');
const sinon = require('sinon');
const {
  commandExists,
  getCommandPath,
  getHelpOutput,
  getVersionOutput,
  commandLookupCommand,
  resolveWindowsCommandSpawn,
  extractNodeScriptFromCmdWrapper,
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

describe('extractNodeScriptFromCmdWrapper', () => {
  it('extracts the node entry script from an npm cmd wrapper', () => {
    const wrapperPath = 'C:\\Users\\sense\\AppData\\Roaming\\npm\\claude.cmd';
    const wrapper = `@ECHO off\r\n"%~dp0\\node.exe"  "%~dp0\\node_modules\\@anthropic-ai\\claude-code\\cli.js" %*\r\n`;

    const scriptPath = extractNodeScriptFromCmdWrapper(wrapper, wrapperPath);
    assert.strictEqual(
      scriptPath,
      path.resolve(path.dirname(wrapperPath), 'node_modules/@anthropic-ai/claude-code/cli.js')
    );
  });

  it('returns null when the wrapper does not reference a node script', () => {
    const wrapper = '@ECHO off\r\necho hello\r\n';
    assert.strictEqual(extractNodeScriptFromCmdWrapper(wrapper, 'C:\\tmp\\tool.cmd'), null);
  });
});

describe('resolveWindowsCommandSpawn', () => {
  let platformStub;
  let tempDir;

  afterEach(() => {
    if (platformStub) platformStub.restore();
    if (tempDir) {
      fs.rmSync(tempDir, { recursive: true, force: true });
      tempDir = null;
    }
  });

  it('invokes node directly for npm cmd wrappers on Windows', () => {
    platformStub = sinon.stub(process, 'platform').value('win32');
    tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'provider-detection-'));
    const wrapperPath = path.join(tempDir, 'claude.cmd');
    const scriptRel = 'node_modules\\@anthropic-ai\\claude-code\\cli.js';
    fs.writeFileSync(
      wrapperPath,
      `@ECHO off\r\n"%~dp0\\node.exe"  "%~dp0\\${scriptRel}" %*\r\n`
    );

    const spec = resolveWindowsCommandSpawn(wrapperPath, ['--print', 'hello world']);
    assert.strictEqual(spec.command, process.execPath);
    assert.strictEqual(
      spec.args[0],
      path.resolve(tempDir, 'node_modules/@anthropic-ai/claude-code/cli.js')
    );
    assert.deepStrictEqual(spec.args.slice(1), ['--print', 'hello world']);
  });

  it('returns the original command on non-Windows platforms', () => {
    platformStub = sinon.stub(process, 'platform').value('linux');
    const spec = resolveWindowsCommandSpawn('claude', ['--print', 'hello']);
    assert.strictEqual(spec.command, 'claude');
    assert.deepStrictEqual(spec.args, ['--print', 'hello']);
  });
});
