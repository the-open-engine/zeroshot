/**
 * E2E test for claudeCommand setting
 * Verifies custom Claude CLI commands are actually invoked with correct arguments
 */
const { expect } = require('chai');
const { spawn } = require('child_process');
const fs = require('fs');
const path = require('path');
const os = require('os');

describe('claudeCommand E2E', function () {
  this.timeout(30000); // E2E tests need more time

  const mockScript = path.join(os.tmpdir(), 'mock-claude-e2e.sh');
  const mockLog = path.join(os.tmpdir(), 'mock-claude-e2e.log');
  const cliPath = path.join(__dirname, '..', 'cli', 'index.js');

  beforeEach(() => {
    // Create mock claude script
    const script = `#!/bin/bash
echo "INVOKED" >> "${mockLog}"
echo "ARGS: $@" >> "${mockLog}"
echo '{"type":"result","result":"mock"}'
exit 0
`;
    fs.writeFileSync(mockScript, script);
    fs.chmodSync(mockScript, '755');

    // Clear log
    if (fs.existsSync(mockLog)) {
      fs.unlinkSync(mockLog);
    }
  });

  afterEach(() => {
    // Cleanup
    if (fs.existsSync(mockScript)) fs.unlinkSync(mockScript);
    if (fs.existsSync(mockLog)) fs.unlinkSync(mockLog);
  });

  it('uses custom command from ZEROSHOT_CLAUDE_COMMAND env var', function (done) {
    const proc = spawn('node', [cliPath, 'task', 'run', 'test prompt'], {
      env: {
        ...process.env,
        ZEROSHOT_CLAUDE_COMMAND: mockScript,
      },
      cwd: __dirname,
    });

    let output = '';
    proc.stdout.on('data', (d) => (output += d.toString()));
    proc.stderr.on('data', (d) => (output += d.toString()));

    proc.on('close', () => {
      // Wait for watcher process to invoke mock
      setTimeout(() => {
        expect(fs.existsSync(mockLog), 'Mock script should have been invoked').to.be.true;
        const log = fs.readFileSync(mockLog, 'utf8');
        expect(log).to.include('INVOKED');
        expect(log).to.include('--print');
        expect(log).to.include('test prompt');
        done();
      }, 2000);
    });
  });

  it('correctly parses space-separated commands like "ccr code"', function (done) {
    const proc = spawn('node', [cliPath, 'task', 'run', 'test prompt'], {
      env: {
        ...process.env,
        ZEROSHOT_CLAUDE_COMMAND: `${mockScript} extra-arg`,
      },
      cwd: __dirname,
    });

    proc.on('close', () => {
      setTimeout(() => {
        expect(fs.existsSync(mockLog)).to.be.true;
        const log = fs.readFileSync(mockLog, 'utf8');
        // extra-arg should appear before --print (prepended to args)
        expect(log).to.include('extra-arg');
        expect(log).to.include('--print');
        done();
      }, 2000);
    });
  });
});
