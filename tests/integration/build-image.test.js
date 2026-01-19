/**
 * Integration test for automatic Docker image building with retry logic
 */

const IsolationManager = require('../../src/isolation-manager');
const { execSync } = require('child_process');

describe('Docker Image Build with Retry', function () {
  // Docker builds can be slow
  this.timeout(120000);

  const TEST_IMAGE = 'zeroshot-cluster-base-test';

  // Skip Docker tests in CI (no Docker image available)
  // To run locally: docker build -t zeroshot-cluster-base docker/zeroshot-cluster/
  before(function () {
    // Opt-in only: building images is slow and environment-dependent.
    if (process.env.ZEROSHOT_DOCKER_IMAGE_TESTS !== '1') {
      this.skip();
      return;
    }

    if (process.env.CI) {
      this.skip();
      return;
    }

    // Check if Docker is available
    if (!IsolationManager.isDockerAvailable()) {
      this.skip();
      return;
    }

    // Clean up test image if exists
    try {
      execSync(`docker rmi -f ${TEST_IMAGE} 2>/dev/null`, { stdio: 'pipe' });
    } catch {
      // Ignore - image doesn't exist
    }
  });

  after(function () {
    // Clean up test image
    try {
      execSync(`docker rmi -f ${TEST_IMAGE} 2>/dev/null`, { stdio: 'pipe' });
    } catch {
      // Ignore
    }
  });

  it('should detect when image does not exist', function () {
    const exists = IsolationManager.imageExists(TEST_IMAGE);
    if (exists !== false) {
      throw new Error('Expected imageExists to return false for non-existent image');
    }
  });

  it('should build image automatically when missing', async function () {
    // Ensure image doesn't exist
    if (IsolationManager.imageExists(TEST_IMAGE)) {
      execSync(`docker rmi -f ${TEST_IMAGE}`, { stdio: 'pipe' });
    }

    // Build image with custom tag
    await IsolationManager.buildImage(TEST_IMAGE);

    // Verify image exists
    const exists = IsolationManager.imageExists(TEST_IMAGE);
    if (!exists) {
      throw new Error('Image should exist after building');
    }
  });

  it('should use ensureImage to auto-build if missing', async function () {
    // Remove image first
    try {
      execSync(`docker rmi -f ${TEST_IMAGE}`, { stdio: 'pipe' });
    } catch {
      // Ignore
    }

    // Ensure image (should auto-build)
    await IsolationManager.ensureImage(TEST_IMAGE, true);

    // Verify image exists
    const exists = IsolationManager.imageExists(TEST_IMAGE);
    if (!exists) {
      throw new Error('ensureImage should have built the image');
    }
  });

  it('should not rebuild if image already exists', async function () {
    // Ensure image exists
    await IsolationManager.ensureImage(TEST_IMAGE, true);

    // Verify image exists
    const exists = IsolationManager.imageExists(TEST_IMAGE);
    if (!exists) {
      throw new Error('Image should exist');
    }

    // Call ensureImage again (should be a no-op)
    const startTime = Date.now();
    await IsolationManager.ensureImage(TEST_IMAGE, true);
    const duration = Date.now() - startTime;

    // Should be very fast if not rebuilding (< 1 second)
    if (duration > 1000) {
      throw new Error('ensureImage should skip rebuild when image exists');
    }
  });

  it('should throw error when autoBuild is false and image missing', async function () {
    const MISSING_IMAGE = 'this-image-does-not-exist';

    try {
      await IsolationManager.ensureImage(MISSING_IMAGE, false);
      throw new Error('Should have thrown error');
    } catch (err) {
      if (!err.message.includes('not found')) {
        throw new Error(`Expected 'not found' error, got: ${err.message}`);
      }
    }
  });
});
