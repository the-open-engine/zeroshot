/**
 * Preflight Validation Tests
 *
 * Tests the preflight checks that run before any cluster/task starts.
 * These tests verify:
 * - Claude CLI detection and auth validation
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

describe('Preflight Validation', function () {
  // Allow slower tests for CLI checks
  this.timeout(10000);

  describe('formatError()', () => {
    it('should format error with title, detail, and recovery steps', () => {
      const result = formatError(
        'Test Error Title',
        'This is the error detail',
        ['Step 1: Do this', 'Step 2: Then this']
      );

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

  describe('getClaudeVersion()', () => {
    it('should detect Claude CLI when installed', function () {
      // Skip if Claude CLI is not installed (CI without Claude)
      try {
        execSync('which claude', { stdio: 'pipe' });
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
  });

  describe('checkGhAuth()', () => {
    it('should detect gh CLI when installed', function () {
      try {
        execSync('which gh', { stdio: 'pipe' });
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

  describe('runPreflight()', () => {
    it('should pass when all required dependencies are available', function () {
      // Create mock valid credentials (works in CI without real auth)
      const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'preflight-test-'));
      const credPath = path.join(tmpDir, '.credentials.json');
      const validCreds = {
        claudeAiOauth: {
          accessToken: 'mock-test-token',
          expiresAt: new Date(Date.now() + 86400000).toISOString(),
        },
      };
      fs.writeFileSync(credPath, JSON.stringify(validCreds));

      const originalDir = process.env.CLAUDE_CONFIG_DIR;
      process.env.CLAUDE_CONFIG_DIR = tmpDir;

      const result = runPreflight({
        requireGh: false,
        requireDocker: false,
        quiet: true,
      });

      process.env.CLAUDE_CONFIG_DIR = originalDir;
      fs.rmSync(tmpDir, { recursive: true });

      expect(result.valid).to.be.true;
      expect(result.errors).to.have.lengthOf(0);
    });

    it('should fail when Claude CLI is not authenticated', () => {
      const originalDir = process.env.CLAUDE_CONFIG_DIR;
      process.env.CLAUDE_CONFIG_DIR = '/nonexistent';

      const result = runPreflight({
        requireGh: false,
        requireDocker: false,
        quiet: true,
      });

      process.env.CLAUDE_CONFIG_DIR = originalDir;

      expect(result.valid).to.be.false;
      expect(result.errors.length).to.be.greaterThan(0);
      expect(result.errors.join('')).to.include('Claude');
    });

    it('should fail when gh CLI required but not authenticated', function () {
      // Skip if gh is not installed
      try {
        execSync('which gh', { stdio: 'pipe' });
      } catch {
        this.skip();
      }

      // Create a scenario where gh auth fails
      const originalHome = process.env.HOME;
      const tmpHome = fs.mkdtempSync(path.join(os.tmpdir(), 'preflight-gh-'));
      process.env.HOME = tmpHome;
      process.env.GH_CONFIG_DIR = path.join(tmpHome, '.config', 'gh');

      const result = runPreflight({
        requireGh: true,
        requireDocker: false,
        quiet: true,
      });

      process.env.HOME = originalHome;
      delete process.env.GH_CONFIG_DIR;
      fs.rmSync(tmpHome, { recursive: true });

      // Either fails because of gh auth or Claude auth
      expect(result.valid).to.be.false;
    });

    it('should include warnings in result', function () {
      // Create mock valid credentials (works in CI without real auth)
      const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'preflight-test-'));
      const credPath = path.join(tmpDir, '.credentials.json');
      const validCreds = {
        claudeAiOauth: {
          accessToken: 'mock-test-token',
          expiresAt: new Date(Date.now() + 86400000).toISOString(),
        },
      };
      fs.writeFileSync(credPath, JSON.stringify(validCreds));

      const originalDir = process.env.CLAUDE_CONFIG_DIR;
      process.env.CLAUDE_CONFIG_DIR = tmpDir;

      const result = runPreflight({
        requireGh: false,
        requireDocker: false,
        quiet: true,
      });

      process.env.CLAUDE_CONFIG_DIR = originalDir;
      fs.rmSync(tmpDir, { recursive: true });

      expect(result).to.have.property('warnings');
      expect(Array.isArray(result.warnings)).to.be.true;
    });
  });

  describe('CLI Integration', function () {
    // Skip in CI - these tests spawn real CLI processes and require Claude CLI to be installed
    before(function () {
      if (process.env.CI) {
        this.skip();
      }
    });

    it('should show preflight passed message on valid run', function () {
      this.timeout(15000);

      // Create mock valid credentials (works in CI without real auth)
      const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'preflight-cli-'));
      const credPath = path.join(tmpDir, '.credentials.json');
      const validCreds = {
        claudeAiOauth: {
          accessToken: 'mock-test-token',
          expiresAt: new Date(Date.now() + 86400000).toISOString(),
        },
      };
      fs.writeFileSync(credPath, JSON.stringify(validCreds));

      const cliPath = path.join(__dirname, '..', 'cli', 'index.js');

      // Run with invalid issue number - preflight should pass, issue fetch should fail
      const result = spawnSync('node', [cliPath, 'run', '999999999'], {
        encoding: 'utf8',
        timeout: 10000,
        env: {
          ...process.env,
          CLAUDE_CONFIG_DIR: tmpDir,
        },
      });

      fs.rmSync(tmpDir, { recursive: true });

      const output = result.stdout + result.stderr;

      // Preflight should pass
      expect(output).to.include('Preflight checks passed');

      // Issue fetch should fail (but after preflight)
      expect(output).to.include('Failed to fetch GitHub issue');
    });

    it('should fail fast when Claude CLI not authenticated', function () {
      this.timeout(10000);

      const cliPath = path.join(__dirname, '..', 'cli', 'index.js');

      const result = spawnSync('node', [cliPath, 'run', '123'], {
        encoding: 'utf8',
        timeout: 8000,
        env: {
          ...process.env,
          CLAUDE_CONFIG_DIR: '/nonexistent',
        },
      });

      const output = result.stdout + result.stderr;

      // Should fail at preflight, not during cluster start
      expect(output).to.include('PREFLIGHT CHECK FAILED');
      expect(output).to.include('Claude CLI');
      expect(output).to.include('claude login');

      // Should NOT get to cluster start
      expect(output).to.not.include('Starting cluster');
    });

    it('should require gh auth when running with issue number', function () {
      this.timeout(10000);

      const cliPath = path.join(__dirname, '..', 'cli', 'index.js');
      const tmpHome = fs.mkdtempSync(path.join(os.tmpdir(), 'preflight-cli-'));

      const result = spawnSync('node', [cliPath, 'run', '123'], {
        encoding: 'utf8',
        timeout: 8000,
        env: {
          ...process.env,
          HOME: tmpHome,
          GH_CONFIG_DIR: path.join(tmpHome, '.config', 'gh'),
          CLAUDE_CONFIG_DIR: '/nonexistent', // Also fail Claude auth
        },
      });

      fs.rmSync(tmpHome, { recursive: true });

      const output = result.stdout + result.stderr;

      // Should fail at preflight
      expect(output).to.include('PREFLIGHT CHECK FAILED');
    });

    it('should not require gh auth when running with plain text', function () {
      this.timeout(10000);

      // Create mock valid Claude credentials (works in CI without real auth)
      const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'preflight-cli-'));
      const credPath = path.join(tmpDir, '.credentials.json');
      const validCreds = {
        claudeAiOauth: {
          accessToken: 'mock-test-token',
          expiresAt: new Date(Date.now() + 86400000).toISOString(),
        },
      };
      fs.writeFileSync(credPath, JSON.stringify(validCreds));

      const cliPath = path.join(__dirname, '..', 'cli', 'index.js');
      const tmpHome = fs.mkdtempSync(path.join(os.tmpdir(), 'preflight-gh-'));

      // Create empty gh config to simulate no gh auth
      fs.mkdirSync(path.join(tmpHome, '.config', 'gh'), { recursive: true });

      const result = spawnSync('node', [cliPath, 'run', '"Test task"'], {
        encoding: 'utf8',
        timeout: 8000,
        cwd: process.cwd(),
        env: {
          ...process.env,
          CLAUDE_CONFIG_DIR: tmpDir,
          GH_CONFIG_DIR: path.join(tmpHome, '.config', 'gh'),
        },
      });

      fs.rmSync(tmpDir, { recursive: true });
      fs.rmSync(tmpHome, { recursive: true });

      const output = result.stdout + result.stderr;

      // Preflight should pass (gh not required for plain text)
      expect(output).to.include('Preflight checks passed');
      // Should NOT fail on gh auth
      expect(output).to.not.include('GitHub CLI (gh) not authenticated');
    });
  });
});

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
