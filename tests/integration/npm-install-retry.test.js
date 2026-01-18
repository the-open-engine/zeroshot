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

let manager;
let tempDir;
let testClusterId;

function writePackageJson(name) {
  const packageJson = {
    name,
    version: '1.0.0',
    dependencies: {},
  };
  fs.writeFileSync(path.join(tempDir, 'package.json'), JSON.stringify(packageJson, null, 2));
}

function registerDockerSetupHooks() {
  before(function () {
    if (process.env.CI) {
      this.skip();
      return;
    }

    if (!IsolationManager.isDockerAvailable()) {
      this.skip();
      return;
    }

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
    tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-npm-retry-test-'));
  });

  afterEach(async function () {
    try {
      await manager.cleanup(testClusterId);
    } catch {
      // Ignore cleanup errors
    }

    if (tempDir && fs.existsSync(tempDir)) {
      fs.rmSync(tempDir, { recursive: true, force: true });
    }
  });
}

function registerRetryMechanismTests() {
  describe('Retry mechanism', function () {
    registerRetryBackoffTest();
    registerRetryMaxAttemptsTest();
    registerNoRetryOnSuccessTest();
    registerSkipWhenMissingPackageJsonTest();
    registerExecutionErrorRetryTest();
  });
}

function registerRetryBackoffTest() {
  it('retries npm install on failure with exponential backoff', async function () {
    writePackageJson('test-retry');

    let callCount = 0;
    manager.execInContainer = function (_clusterId, _command, _options) {
      callCount++;

      if (callCount <= 2) {
        return {
          code: 1,
          stdout: '',
          stderr: 'npm ERR! network timeout',
        };
      }

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

    assert.strictEqual(callCount, 3, 'Should retry twice after initial failure');
    assert(elapsed >= 6000, `Should wait at least 6s between retries, got ${elapsed}ms`);
  });
}

function registerRetryMaxAttemptsTest() {
  it('fails after max retries exceeded', async function () {
    writePackageJson('test-retry-fail');

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

    await manager.createContainer(testClusterId, {
      workDir: tempDir,
      image: 'alpine:latest',
    });

    const elapsed = Date.now() - startTime;

    assert.strictEqual(callCount, 3, 'Should attempt 3 times total');
    assert(elapsed >= 6000, `Should wait at least 6s for all retries, got ${elapsed}ms`);
  });
}

function registerNoRetryOnSuccessTest() {
  it('does not retry if first attempt succeeds', async function () {
    writePackageJson('test-no-retry');

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

    assert.strictEqual(callCount, 1, 'Should only attempt once on success');
    assert(elapsed < 5000, `Should complete quickly without retries, got ${elapsed}ms`);
  });
}

function registerSkipWhenMissingPackageJsonTest() {
  it('skips npm install if no package.json exists', async function () {
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

    assert.strictEqual(callCount, 0, 'Should not attempt npm install without package.json');
  });
}

function registerExecutionErrorRetryTest() {
  it('handles execution errors during retry', async function () {
    writePackageJson('test-exec-error');

    let callCount = 0;
    manager.execInContainer = function () {
      callCount++;

      if (callCount === 1) {
        throw new Error('Container disconnected');
      }
      if (callCount === 2) {
        return {
          code: 1,
          stdout: '',
          stderr: 'npm ERR! network timeout',
        };
      }

      return {
        code: 0,
        stdout: 'added 0 packages',
        stderr: '',
      };
    };

    await manager.createContainer(testClusterId, {
      workDir: tempDir,
      image: 'alpine:latest',
    });

    assert.strictEqual(callCount, 3, 'Should retry after execution errors');
  });
}

function registerExponentialBackoffTimingTests() {
  describe('Exponential backoff timing', function () {
    it('uses correct delay calculation: 2s, 4s, 8s', async function () {
      writePackageJson('test-timing');

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
        }
        return {
          code: 0,
          stdout: 'ok',
          stderr: '',
        };
      };

      await manager.createContainer(testClusterId, {
        workDir: tempDir,
        image: 'alpine:latest',
      });

      assert.strictEqual(attemptTimes.length, 3, 'Should have 3 attempts');

      const delay1 = attemptTimes[1] - attemptTimes[0];
      const delay2 = attemptTimes[2] - attemptTimes[1];

      assert(delay1 >= 2000 && delay1 < 3000, `First delay should be ~2s, got ${delay1}ms`);
      assert(delay2 >= 4000 && delay2 < 5000, `Second delay should be ~4s, got ${delay2}ms`);
    });
  });
}

function registerNonFatalFailureTests() {
  describe('Non-fatal failure behavior', function () {
    it('logs warning but continues when all retries fail', async function () {
      writePackageJson('test-non-fatal');

      const originalWarn = console.warn;
      const warnings = [];
      console.warn = (...args) => {
        warnings.push(args.join(' '));
      };

      manager.execInContainer = function () {
        return {
          code: 1,
          stdout: '',
          stderr: 'npm ERR! epic fail',
        };
      };

      try {
        const containerId = await manager.createContainer(testClusterId, {
          workDir: tempDir,
          image: 'alpine:latest',
        });

        assert(containerId, 'Should return container ID even if npm install fails');
        assert(manager.hasContainer(testClusterId), 'Container should exist');

        const failureWarnings = warnings.filter((w) => w.includes('npm install failed'));
        assert(failureWarnings.length > 0, 'Should log warnings about npm install failures');
      } finally {
        console.warn = originalWarn;
      }
    });
  });
}

describe('npm install retry logic', function () {
  this.timeout(120000); // 2 minutes - retries can take a while

  registerDockerSetupHooks();
  registerRetryMechanismTests();
  registerExponentialBackoffTimingTests();
  registerNonFatalFailureTests();
});
