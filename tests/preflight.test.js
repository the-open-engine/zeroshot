/**
 * Preflight Validation Tests
 *
 * Tests the preflight checks that run before any cluster/task starts.
 * These tests verify:
 * - Provider CLI detection
 * - gh CLI detection and auth validation
 * - Docker availability detection
 * - Error message formatting
 * - Integration with CLI entry points
 */

const { expect } = require('chai');
const { execSync, spawnSync } = require('child_process');
const path = require('path');
const fs = require('fs');
const os = require('os');

const {
  runPreflight,
  getClaudeVersion,
  checkClaudeAuth,
  checkGhAuth,
  checkDocker,
  formatError,
} = require('../src/preflight');

// Cross-platform helper for checking if command exists
const whichCmd = process.platform === 'win32' ? 'where' : 'which';

describe('Preflight Validation', function () {
  // Allow slower tests for CLI checks
  this.timeout(10000);

  defineFormatErrorTests();
  defineClaudeVersionTests();
  defineClaudeAuthTests();
  defineGhAuthTests();
  defineDockerTests();
  defineRunPreflightTests();
  defineCliIntegrationTests();
});

function defineFormatErrorTests() {
  describe('formatError()', () => {
    it('should format error with title, detail, and recovery steps', () => {
      const result = formatError('Test Error Title', 'This is the error detail', [
        'Step 1: Do this',
        'Step 2: Then this',
      ]);

      expect(result).to.include('❌ Test Error Title');
      expect(result).to.include('This is the error detail');
      expect(result).to.include('To fix:');
      expect(result).to.include('1. Step 1: Do this');
      expect(result).to.include('2. Step 2: Then this');
    });

    it('should handle empty recovery steps', () => {
      const result = formatError('Error', 'Detail', []);
      expect(result).to.include('❌ Error');
      expect(result).to.include('Detail');
      expect(result).to.not.include('To fix:');
    });
  });
}

function defineClaudeVersionTests() {
  describe('getClaudeVersion()', () => {
    it('should detect Claude CLI when installed', function () {
      // Skip if Claude CLI is not installed (CI without Claude)
      try {
        execSync(`${whichCmd} claude`, { stdio: 'pipe' });
      } catch {
        this.skip();
      }

      const result = getClaudeVersion();
      expect(result.installed).to.be.true;
      expect(result.version).to.match(/^\d+\.\d+\.\d+$|^unknown$/);
      expect(result.error).to.be.null;
    });

    it('should report not installed when Claude CLI is missing', () => {
      // Test with a fake PATH that excludes claude
      const originalPath = process.env.PATH;
      process.env.PATH = '/nonexistent';

      const result = getClaudeVersion();

      process.env.PATH = originalPath;

      expect(result.installed).to.be.false;
      expect(result.version).to.be.null;
    });
  });
}

function defineClaudeAuthTests() {
  describe('checkClaudeAuth()', () => {
    it('should detect authentication when credentials exist', function () {
      const configDir = process.env.CLAUDE_CONFIG_DIR || path.join(os.homedir(), '.claude');
      const credPath = path.join(configDir, '.credentials.json');

      // Skip if no credentials file
      if (!fs.existsSync(credPath)) {
        this.skip();
      }

      const result = checkClaudeAuth();
      expect(result.configDir).to.equal(configDir);
      // May be authenticated or not depending on token expiry
      expect(result).to.have.property('authenticated');
      expect(result).to.have.property('error');
    });

    it('should report unauthenticated when config dir does not exist', () => {
      const originalDir = process.env.CLAUDE_CONFIG_DIR;
      process.env.CLAUDE_CONFIG_DIR = '/nonexistent/path/.claude';

      const result = checkClaudeAuth();

      process.env.CLAUDE_CONFIG_DIR = originalDir;

      expect(result.authenticated).to.be.false;
      expect(result.error).to.include('No credentials file found');
      expect(result.configDir).to.equal('/nonexistent/path/.claude');
    });

    it('should report unauthenticated when credentials file is empty', () => {
      const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'preflight-test-'));
      const credPath = path.join(tmpDir, '.credentials.json');
      fs.writeFileSync(credPath, '{}');

      const originalDir = process.env.CLAUDE_CONFIG_DIR;
      process.env.CLAUDE_CONFIG_DIR = tmpDir;

      const result = checkClaudeAuth();

      process.env.CLAUDE_CONFIG_DIR = originalDir;
      fs.rmSync(tmpDir, { recursive: true });

      expect(result.authenticated).to.be.false;
      expect(result.error).to.include('No valid authentication');
    });

    it('should detect expired OAuth token', () => {
      const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'preflight-test-'));
      const credPath = path.join(tmpDir, '.credentials.json');

      // Create credentials with expired token
      const expiredCreds = {
        claudeAiOauth: {
          accessToken: 'test-token',
          expiresAt: new Date(Date.now() - 86400000).toISOString(), // Yesterday
        },
      };
      fs.writeFileSync(credPath, JSON.stringify(expiredCreds));

      const originalDir = process.env.CLAUDE_CONFIG_DIR;
      process.env.CLAUDE_CONFIG_DIR = tmpDir;

      const result = checkClaudeAuth();

      process.env.CLAUDE_CONFIG_DIR = originalDir;
      fs.rmSync(tmpDir, { recursive: true });

      expect(result.authenticated).to.be.false;
      expect(result.error).to.include('expired');
    });

    it('should accept valid OAuth token', () => {
      const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'preflight-test-'));
      const credPath = path.join(tmpDir, '.credentials.json');

      // Create credentials with valid token
      const validCreds = {
        claudeAiOauth: {
          accessToken: 'test-token',
          expiresAt: new Date(Date.now() + 86400000).toISOString(), // Tomorrow
        },
      };
      fs.writeFileSync(credPath, JSON.stringify(validCreds));

      const originalDir = process.env.CLAUDE_CONFIG_DIR;
      process.env.CLAUDE_CONFIG_DIR = tmpDir;

      const result = checkClaudeAuth();

      process.env.CLAUDE_CONFIG_DIR = originalDir;
      fs.rmSync(tmpDir, { recursive: true });

      expect(result.authenticated).to.be.true;
      expect(result.error).to.be.null;
    });

    it('should accept API key authentication', () => {
      const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'preflight-test-'));
      const credPath = path.join(tmpDir, '.credentials.json');

      const apiKeyCreds = {
        apiKey: 'sk-ant-test-key',
      };
      fs.writeFileSync(credPath, JSON.stringify(apiKeyCreds));

      const originalDir = process.env.CLAUDE_CONFIG_DIR;
      process.env.CLAUDE_CONFIG_DIR = tmpDir;

      const result = checkClaudeAuth();

      process.env.CLAUDE_CONFIG_DIR = originalDir;
      fs.rmSync(tmpDir, { recursive: true });

      expect(result.authenticated).to.be.true;
      expect(result.error).to.be.null;
    });

    it('should detect authentication via macOS Keychain when credentials file missing', function () {
      // Skip if not macOS
      if (os.platform() !== 'darwin') {
        this.skip();
      }

      // Skip if Claude credentials not in Keychain
      try {
        execSync('security find-generic-password -s "Claude Code-credentials"', {
          stdio: 'pipe',
          timeout: 2000,
        });
      } catch {
        this.skip();
      }

      // Skip if credentials file exists (can't test Keychain fallback)
      const defaultCredPath = path.join(os.homedir(), '.claude', '.credentials.json');
      if (fs.existsSync(defaultCredPath)) {
        this.skip();
      }

      // Must NOT set CLAUDE_CONFIG_DIR for Keychain fallback to activate
      const originalDir = process.env.CLAUDE_CONFIG_DIR;
      delete process.env.CLAUDE_CONFIG_DIR;

      const result = checkClaudeAuth();

      if (originalDir) {
        process.env.CLAUDE_CONFIG_DIR = originalDir;
      }

      expect(result.authenticated).to.be.true;
      expect(result.method).to.equal('keychain');
    });
  });
}

function defineGhAuthTests() {
  describe('checkGhAuth()', () => {
    it('should detect gh CLI when installed', function () {
      try {
        execSync(`${whichCmd} gh`, { stdio: 'pipe' });
      } catch {
        this.skip();
      }

      const result = checkGhAuth();
      expect(result.installed).to.be.true;
      // Auth status depends on environment
      expect(result).to.have.property('authenticated');
    });

    it('should report not installed when gh CLI is missing', () => {
      const originalPath = process.env.PATH;
      process.env.PATH = '/nonexistent';

      const result = checkGhAuth();

      process.env.PATH = originalPath;

      expect(result.installed).to.be.false;
      expect(result.authenticated).to.be.false;
      expect(result.error).to.include('not installed');
    });
  });
}

function defineDockerTests() {
  describe('checkDocker()', () => {
    it('should detect Docker when available', function () {
      try {
        execSync('docker --version', { stdio: 'pipe' });
      } catch {
        this.skip();
      }

      const result = checkDocker();
      // Docker installed, but daemon might not be running
      expect(result).to.have.property('available');
      expect(result).to.have.property('error');
    });

    it('should report not available when Docker is missing', () => {
      const originalPath = process.env.PATH;
      process.env.PATH = '/nonexistent';

      const result = checkDocker();

      process.env.PATH = originalPath;

      expect(result.available).to.be.false;
      expect(result.error).to.include('not installed');
    });
  });
}

function defineRunPreflightTests() {
  describe('runPreflight()', () => {
    it('should fail when Claude CLI is missing', () => {
      const originalPath = process.env.PATH;
      process.env.PATH = '/nonexistent';

      const result = runPreflight({
        requireGh: false,
        requireDocker: false,
        quiet: true,
        provider: 'claude',
      });

      process.env.PATH = originalPath;

      expect(result.valid).to.be.false;
      expect(result.errors.join('')).to.include('Claude command not available');
    });

    it('should fail when Codex CLI is missing', () => {
      const originalPath = process.env.PATH;
      process.env.PATH = '/nonexistent';

      const result = runPreflight({
        requireGh: false,
        requireDocker: false,
        quiet: true,
        provider: 'codex',
      });

      process.env.PATH = originalPath;

      expect(result.valid).to.be.false;
      expect(result.errors.join('')).to.include('Codex CLI not available');
    });

    it('should fail when Gemini CLI is missing', () => {
      const originalPath = process.env.PATH;
      process.env.PATH = '/nonexistent';

      const result = runPreflight({
        requireGh: false,
        requireDocker: false,
        quiet: true,
        provider: 'gemini',
      });

      process.env.PATH = originalPath;

      expect(result.valid).to.be.false;
      expect(result.errors.join('')).to.include('Gemini CLI not available');
    });

    it('should not require Claude auth when CLI is installed', function () {
      try {
        execSync(`${whichCmd} claude`, { stdio: 'pipe' });
      } catch {
        this.skip();
      }
      if (process.getuid && process.getuid() === 0) {
        this.skip();
      }

      const originalDir = process.env.CLAUDE_CONFIG_DIR;
      process.env.CLAUDE_CONFIG_DIR = '/nonexistent';

      const result = runPreflight({
        requireGh: false,
        requireDocker: false,
        quiet: true,
        provider: 'claude',
      });

      process.env.CLAUDE_CONFIG_DIR = originalDir;

      expect(result.valid).to.be.true;
    });

    it('should include warnings array in result', function () {
      const result = runPreflight({
        requireGh: false,
        requireDocker: false,
        quiet: true,
        provider: 'codex',
      });

      expect(result).to.have.property('warnings');
      expect(Array.isArray(result.warnings)).to.be.true;
    });
  });
}

function defineCliIntegrationTests() {
  describe('CLI Integration', function () {
    it('should fail fast when provider CLI is missing', function () {
      this.timeout(10000);

      const cliPath = path.join(__dirname, '..', 'cli', 'index.js');
      const result = spawnSync(process.execPath, [cliPath, 'run', 'Test task'], {
        encoding: 'utf8',
        timeout: 8000,
        env: {
          ...process.env,
          PATH: '/nonexistent',
          ZEROSHOT_PROVIDER: 'codex',
        },
      });

      const output = result.stdout + result.stderr;

      expect(output).to.include('PREFLIGHT CHECK FAILED');
      expect(output).to.include('Codex CLI not available');
    });
  });
}

describe('Preflight in Container Environment', () => {
  it('should work with mounted credentials', function () {
    this.timeout(5000);

    // Simulate container environment with mounted credentials
    const tmpMount = fs.mkdtempSync(path.join(os.tmpdir(), 'container-creds-'));
    const credPath = path.join(tmpMount, '.credentials.json');

    // Create valid credentials
    const creds = {
      claudeAiOauth: {
        accessToken: 'container-test-token',
        expiresAt: new Date(Date.now() + 86400000).toISOString(),
      },
    };
    fs.writeFileSync(credPath, JSON.stringify(creds));

    const originalDir = process.env.CLAUDE_CONFIG_DIR;
    process.env.CLAUDE_CONFIG_DIR = tmpMount;

    const result = checkClaudeAuth();

    process.env.CLAUDE_CONFIG_DIR = originalDir;
    fs.rmSync(tmpMount, { recursive: true });

    expect(result.authenticated).to.be.true;
    expect(result.configDir).to.equal(tmpMount);
  });
});
