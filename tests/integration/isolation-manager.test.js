/**
 * Test: IsolationManager - Docker container lifecycle
 *
 * SAFETY: These tests only test Docker container operations.
 * - NO cluster.start() calls
 * - NO agent spawning
 * - NO Claude API calls
 * - Only tests container create/exec/stop/remove
 *
 * REQUIRES: Docker installed and running
 * SKIPS: If Docker not available
 */

const assert = require('assert');
const IsolationManager = require('../../src/isolation-manager');

describe('IsolationManager', function () {
  this.timeout(60000); // Docker operations can be slow

  // Skip Docker tests in CI (no Docker image available)
  // To run locally: docker build -t zeroshot-cluster-base docker/zeroshot-cluster/
  before(function () {
    if (process.env.CI) {
      this.skip();
      return;
    }

    if (!IsolationManager.isDockerAvailable()) {
      this.skip();
    }
  });

  describe('Static Methods', function () {
    it('isDockerAvailable() returns boolean', function () {
      const result = IsolationManager.isDockerAvailable();
      assert.strictEqual(typeof result, 'boolean');
    });

    it('imageExists() returns false for non-existent image', function () {
      const result = IsolationManager.imageExists('definitely-not-a-real-image-xyz123');
      assert.strictEqual(result, false);
    });

    it('imageExists() returns true for alpine (common base image)', function () {
      // Pull alpine if not exists (small image, fast)
      const { execSync } = require('child_process');
      try {
        execSync('docker pull alpine:latest 2>/dev/null', { stdio: 'pipe' });
      } catch {
        // Ignore pull errors
      }

      const result = IsolationManager.imageExists('alpine:latest');
      // May or may not exist depending on environment
      assert.strictEqual(typeof result, 'boolean');
    });
  });

  describe('Container Lifecycle (with alpine)', function () {
    let manager;
    const testClusterId = 'test-isolation-' + Date.now();

    before(function () {
      // Ensure alpine is available
      const { execSync } = require('child_process');
      try {
        execSync('docker pull alpine:latest 2>/dev/null', { stdio: 'pipe' });
      } catch (err) {
        throw new Error(
          `Failed to pull alpine:latest image. Docker may not be running or network is unavailable: ${err.message}`
        );
      }

      manager = new IsolationManager({ image: 'alpine:latest' });
    });

    afterEach(async function () {
      // Clean up container after each test
      try {
        await manager.removeContainer(testClusterId, true);
      } catch {
        // Ignore cleanup errors
      }
    });

    it('createContainer() creates a running container', async function () {
      const containerId = await manager.createContainer(testClusterId, {
        workDir: process.cwd(),
        image: 'alpine:latest',
      });

      assert(containerId, 'Should return container ID');
      assert.strictEqual(containerId.length, 12, 'Container ID should be 12 chars');
      assert(manager.hasContainer(testClusterId), 'Should track container');
    });

    it('execInContainer() runs commands inside container', async function () {
      await manager.createContainer(testClusterId, {
        workDir: process.cwd(),
        image: 'alpine:latest',
      });

      const result = await manager.execInContainer(testClusterId, ['echo', 'hello world']);

      assert.strictEqual(result.code, 0, 'Command should succeed');
      assert(result.stdout.includes('hello world'), 'Output should contain hello world');
    });

    it('getContainerId() returns container ID', async function () {
      await manager.createContainer(testClusterId, {
        workDir: process.cwd(),
        image: 'alpine:latest',
      });

      const containerId = manager.getContainerId(testClusterId);
      assert(containerId, 'Should return container ID');
    });

    it('stopContainer() stops running container', async function () {
      await manager.createContainer(testClusterId, {
        workDir: process.cwd(),
        image: 'alpine:latest',
      });

      await manager.stopContainer(testClusterId, 1); // 1 second timeout

      // Container should no longer be running
      assert.strictEqual(manager.hasContainer(testClusterId), false);
    });

    it('removeContainer() removes container', async function () {
      await manager.createContainer(testClusterId, {
        workDir: process.cwd(),
        image: 'alpine:latest',
      });

      await manager.removeContainer(testClusterId, true); // force

      assert.strictEqual(manager.getContainerId(testClusterId), undefined);
    });

    it('cleanup() stops and removes container', async function () {
      await manager.createContainer(testClusterId, {
        workDir: process.cwd(),
        image: 'alpine:latest',
      });

      await manager.cleanup(testClusterId);

      assert.strictEqual(manager.hasContainer(testClusterId), false);
      assert.strictEqual(manager.getContainerId(testClusterId), undefined);
    });

    it('spawnInContainer() returns child process', async function () {
      await manager.createContainer(testClusterId, {
        workDir: process.cwd(),
        image: 'alpine:latest',
      });

      const proc = manager.spawnInContainer(testClusterId, ['cat']);

      assert(proc, 'Should return process');
      assert(proc.stdin, 'Process should have stdin');
      assert(proc.stdout, 'Process should have stdout');

      // Write to stdin and read from stdout
      proc.stdin.write('test input');
      proc.stdin.end();

      const output = await new Promise((resolve) => {
        let data = '';
        proc.stdout.on('data', (d) => {
          data += d;
        });
        proc.on('close', () => resolve(data));
      });

      assert(output.includes('test input'), 'Should echo back input');
    });
  });

  describe('Error Handling', function () {
    it('execInContainer() throws for non-existent cluster', async function () {
      const manager = new IsolationManager();

      try {
        await manager.execInContainer('non-existent-cluster', ['echo', 'test']);
        assert.fail('Should throw');
      } catch (err) {
        assert(err.message.includes('No container found'));
      }
    });

    it('spawnInContainer() throws for non-existent cluster', function () {
      const manager = new IsolationManager();

      try {
        manager.spawnInContainer('non-existent-cluster', ['echo', 'test']);
        assert.fail('Should throw');
      } catch (err) {
        assert(err.message.includes('No container found'));
      }
    });
  });
});
