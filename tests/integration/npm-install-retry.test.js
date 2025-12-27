/**
 * Test: npm install retry logic in IsolationManager
 *
 * Tests the exponential backoff retry mechanism for npm install failures
 * during container creation.
 *
 * REQUIRES: Docker installed and running
 * SKIPS: If Docker not available
 */

const assert = require('assert');
const IsolationManager = require('../../src/isolation-manager');
const path = require('path');
const fs = require('fs');
const os = require('os');

describe('npm install retry logic', function () {
  this.timeout(120000); // 2 minutes - retries can take a while

  let manager;
  let tempDir;
  let testClusterId;

  before(function () {
    if (!IsolationManager.isDockerAvailable()) {
      throw new Error(
        'Docker is required to run IsolationManager tests. Install Docker and try again.'
      );
    }

    // Ensure alpine is available (lightweight image for tests)
    const { execSync } = require('child_process');
    try {
      execSync('docker pull alpine:latest 2>/dev/null', { stdio: 'pipe' });
    } catch (err) {
      throw new Error(
        `Failed to pull alpine:latest image. Docker may not be running or network is unavailable: ${err.message}`
      );
    }
  });

  beforeEach(function () {
    manager = new IsolationManager({ image: 'alpine:latest' });
    testClusterId = 'test-npm-retry-' + Date.now();

    // Create temp directory with package.json
    tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-npm-retry-test-'));
  });

  afterEach(async function () {
    // Clean up container
    try {
      await manager.cleanup(testClusterId);
    } catch {
      // Ignore cleanup errors
    }

    // Clean up temp directory
    if (tempDir && fs.existsSync(tempDir)) {
      fs.rmSync(tempDir, { recursive: true, force: true });
    }
  });

  describe('Retry mechanism', function () {
    it('retries npm install on failure with exponential backoff', async function () {
      // Create a package.json to trigger npm install
      const packageJson = {
        name: 'test-retry',
        version: '1.0.0',
        dependencies: {},
      };
      fs.writeFileSync(path.join(tempDir, 'package.json'), JSON.stringify(packageJson, null, 2));

      // Mock execInContainer to fail twice, then succeed
      let callCount = 0;
      manager.execInContainer = function (_clusterId, _command, _options) {
        callCount++;

        if (callCount <= 2) {
          // First two attempts fail
          return {
            code: 1,
            stdout: '',
            stderr: 'npm ERR! network timeout',
          };
        } else {
          // Third attempt succeeds
          return {
            code: 0,
            stdout: 'added 0 packages',
            stderr: '',
          };
        }
      };

      const startTime = Date.now();
      await manager.createContainer(testClusterId, {
        workDir: tempDir,
        image: 'alpine:latest',
      });
      const elapsed = Date.now() - startTime;

      // Should have made 3 attempts
      assert.strictEqual(callCount, 3, 'Should retry twice after initial failure');

      // Verify exponential backoff delays (2s, 4s)
      // Total delay should be at least 6 seconds (2s + 4s)
      assert(elapsed >= 6000, `Should wait at least 6s between retries, got ${elapsed}ms`);
    });

    it('fails after max retries exceeded', async function () {
      // Create a package.json to trigger npm install
      const packageJson = {
        name: 'test-retry-fail',
        version: '1.0.0',
        dependencies: {},
      };
      fs.writeFileSync(path.join(tempDir, 'package.json'), JSON.stringify(packageJson, null, 2));

      // Mock execInContainer to always fail
      let callCount = 0;
      manager.execInContainer = function () {
        callCount++;
        return {
          code: 1,
          stdout: '',
          stderr: 'npm ERR! network timeout',
        };
      };

      const startTime = Date.now();

      // Should NOT throw - npm install failure is non-fatal
      await manager.createContainer(testClusterId, {
        workDir: tempDir,
        image: 'alpine:latest',
      });

      const elapsed = Date.now() - startTime;

      // Should have made 3 attempts (initial + 2 retries)
      assert.strictEqual(callCount, 3, 'Should attempt 3 times total');

      // Verify exponential backoff delays (2s before attempt 2, 4s before attempt 3)
      // Total delay should be at least 6 seconds (2s + 4s = 6s, no delay after last attempt)
      assert(elapsed >= 6000, `Should wait at least 6s for all retries, got ${elapsed}ms`);
    });

    it('does not retry if first attempt succeeds', async function () {
      // Create a package.json to trigger npm install
      const packageJson = {
        name: 'test-no-retry',
        version: '1.0.0',
        dependencies: {},
      };
      fs.writeFileSync(path.join(tempDir, 'package.json'), JSON.stringify(packageJson, null, 2));

      // Mock execInContainer to succeed immediately
      let callCount = 0;
      manager.execInContainer = function () {
        callCount++;
        return {
          code: 0,
          stdout: 'added 0 packages',
          stderr: '',
        };
      };

      const startTime = Date.now();
      await manager.createContainer(testClusterId, {
        workDir: tempDir,
        image: 'alpine:latest',
      });
      const elapsed = Date.now() - startTime;

      // Should only call once
      assert.strictEqual(callCount, 1, 'Should only attempt once on success');

      // Should not have significant delays
      assert(elapsed < 5000, `Should complete quickly without retries, got ${elapsed}ms`);
    });

    it('skips npm install if no package.json exists', async function () {
      // Don't create package.json

      // Mock execInContainer (should never be called)
      let callCount = 0;
      const originalExec = manager.execInContainer.bind(manager);
      manager.execInContainer = function (...args) {
        callCount++;
        return originalExec(...args);
      };

      await manager.createContainer(testClusterId, {
        workDir: tempDir,
        image: 'alpine:latest',
      });

      // Should not have called execInContainer
      assert.strictEqual(callCount, 0, 'Should not attempt npm install without package.json');
    });

    it('handles execution errors during retry', async function () {
      // Create a package.json to trigger npm install
      const packageJson = {
        name: 'test-exec-error',
        version: '1.0.0',
        dependencies: {},
      };
      fs.writeFileSync(path.join(tempDir, 'package.json'), JSON.stringify(packageJson, null, 2));

      // Mock execInContainer to throw errors first, then succeed
      let callCount = 0;
      manager.execInContainer = function () {
        callCount++;

        if (callCount === 1) {
          // First attempt throws error
          throw new Error('Container disconnected');
        } else if (callCount === 2) {
          // Second attempt returns failure
          return {
            code: 1,
            stdout: '',
            stderr: 'npm ERR! network timeout',
          };
        } else {
          // Third attempt succeeds
          return {
            code: 0,
            stdout: 'added 0 packages',
            stderr: '',
          };
        }
      };

      await manager.createContainer(testClusterId, {
        workDir: tempDir,
        image: 'alpine:latest',
      });

      // Should have made 3 attempts
      assert.strictEqual(callCount, 3, 'Should retry after execution errors');
    });
  });

  describe('Exponential backoff timing', function () {
    it('uses correct delay calculation: 2s, 4s, 8s', async function () {
      // Create a package.json to trigger npm install
      const packageJson = {
        name: 'test-timing',
        version: '1.0.0',
        dependencies: {},
      };
      fs.writeFileSync(path.join(tempDir, 'package.json'), JSON.stringify(packageJson, null, 2));

      const attemptTimes = [];
      let callCount = 0;

      manager.execInContainer = function () {
        attemptTimes.push(Date.now());
        callCount++;

        if (callCount <= 2) {
          return {
            code: 1,
            stdout: '',
            stderr: 'npm ERR! fail',
          };
        } else {
          return {
            code: 0,
            stdout: 'ok',
            stderr: '',
          };
        }
      };

      await manager.createContainer(testClusterId, {
        workDir: tempDir,
        image: 'alpine:latest',
      });

      // Verify delays between attempts
      assert.strictEqual(attemptTimes.length, 3, 'Should have 3 attempts');

      const delay1 = attemptTimes[1] - attemptTimes[0];
      const delay2 = attemptTimes[2] - attemptTimes[1];

      // First delay should be around 2000ms (2s * 2^0)
      assert(delay1 >= 2000 && delay1 < 3000, `First delay should be ~2s, got ${delay1}ms`);

      // Second delay should be around 4000ms (2s * 2^1)
      assert(delay2 >= 4000 && delay2 < 5000, `Second delay should be ~4s, got ${delay2}ms`);
    });
  });

  describe('Non-fatal failure behavior', function () {
    it('logs warning but continues when all retries fail', async function () {
      // Create a package.json to trigger npm install
      const packageJson = {
        name: 'test-non-fatal',
        version: '1.0.0',
        dependencies: {},
      };
      fs.writeFileSync(path.join(tempDir, 'package.json'), JSON.stringify(packageJson, null, 2));

      // Mock console.warn to capture warnings
      const originalWarn = console.warn;
      const warnings = [];
      console.warn = (...args) => {
        warnings.push(args.join(' '));
      };

      // Mock execInContainer to always fail
      manager.execInContainer = function () {
        return {
          code: 1,
          stdout: '',
          stderr: 'npm ERR! epic fail',
        };
      };

      try {
        // Should NOT throw
        const containerId = await manager.createContainer(testClusterId, {
          workDir: tempDir,
          image: 'alpine:latest',
        });

        // Container should still be created
        assert(containerId, 'Should return container ID even if npm install fails');
        assert(manager.hasContainer(testClusterId), 'Container should exist');

        // Should have logged warnings
        const failureWarnings = warnings.filter((w) => w.includes('npm install failed'));
        assert(failureWarnings.length > 0, 'Should log warnings about npm install failures');
      } finally {
        console.warn = originalWarn;
      }
    });
  });
});
