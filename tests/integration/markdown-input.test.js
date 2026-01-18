/**
 * Integration Test: Markdown File Input
 *
 * End-to-end test for markdown file input feature:
 * 1. Create temp markdown file
 * 2. Start cluster with file input
 * 3. Verify ISSUE_OPENED message contains markdown content
 * 4. Verify conductor receives markdown in context
 * 5. Cleanup
 */

const assert = require('node:assert');
const path = require('path');
const fs = require('fs');
const os = require('os');

const Orchestrator = require('../../src/orchestrator');
const _Ledger = require('../../src/ledger');
const MockTaskRunner = require('../helpers/mock-task-runner');

let tempDir;
let orchestrator;
let mockRunner;

const simpleConfig = {
  agents: [
    {
      id: 'worker',
      role: 'implementation',
      timeout: 0,
      triggers: [{ topic: 'ISSUE_OPENED', action: 'execute_task' }],
      prompt: 'Process the markdown input',
      hooks: {
        onComplete: {
          action: 'publish_message',
          config: { topic: 'TASK_COMPLETE', content: { text: 'Done' } },
        },
      },
    },
    {
      id: 'completion-detector',
      role: 'orchestrator',
      timeout: 0,
      triggers: [{ topic: 'TASK_COMPLETE', action: 'stop_cluster' }],
    },
  ],
};

async function waitForClusterState(orch, clusterId, targetState, timeoutMs = 10000) {
  const startTime = Date.now();
  while (Date.now() - startTime < timeoutMs) {
    const cluster = orch.getCluster(clusterId);
    if (!cluster) {
      throw new Error(`Cluster ${clusterId} not found`);
    }
    if (cluster.state === targetState) {
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`Cluster did not reach state "${targetState}" within ${timeoutMs}ms`);
}

function registerBasicMarkdownInputTests() {
  describe('Basic markdown file input', function () {
    it('should read markdown file and pass content to cluster', async function () {
      // Create test markdown file
      const markdownPath = path.join(tempDir, 'feature.md');
      const markdownContent = '# Add Dark Mode\n\nImplement dark mode toggle in settings.';
      fs.writeFileSync(markdownPath, markdownContent);

      mockRunner.when('worker').returns(JSON.stringify({ summary: 'Processed' }));

      orchestrator = new Orchestrator({
        quiet: true,
        storageDir: tempDir,
        taskRunner: mockRunner,
      });

      const result = await orchestrator.start(simpleConfig, { file: markdownPath });
      const clusterId = result.id;

      // Wait for completion
      await waitForClusterState(orchestrator, clusterId, 'stopped', 10000);

      // Verify worker received markdown content
      mockRunner.assertCalled('worker', 1);
      const calls = mockRunner.getCalls('worker');
      assert(calls[0].context.includes('Add Dark Mode'), 'Context should include markdown title');
      assert(
        calls[0].context.includes('dark mode toggle'),
        'Context should include markdown content'
      );
    });

    it('should extract title from # header in markdown', async function () {
      const markdownPath = path.join(tempDir, 'test.md');
      const markdownContent = '# Feature Request\n\nAdd user authentication.';
      fs.writeFileSync(markdownPath, markdownContent);

      mockRunner.when('worker').returns(JSON.stringify({ summary: 'Done' }));

      orchestrator = new Orchestrator({
        quiet: true,
        storageDir: tempDir,
        taskRunner: mockRunner,
      });

      const result = await orchestrator.start(simpleConfig, { file: markdownPath });
      const clusterId = result.id;

      await waitForClusterState(orchestrator, clusterId, 'stopped', 10000);

      // Check ISSUE_OPENED message
      const ledger = new _Ledger(path.join(tempDir, `${clusterId}.db`));
      const messages = ledger.query({ cluster_id: clusterId, topic: 'ISSUE_OPENED' });
      assert.strictEqual(messages.length, 1);
      assert(messages[0].content.text.includes('Feature Request'));
      ledger.close();
    });

    it('should use filename as title when no header present', async function () {
      const markdownPath = path.join(tempDir, 'my-feature.md');
      const markdownContent = 'This markdown has no header.';
      fs.writeFileSync(markdownPath, markdownContent);

      mockRunner.when('worker').returns(JSON.stringify({ summary: 'Done' }));

      orchestrator = new Orchestrator({
        quiet: true,
        storageDir: tempDir,
        taskRunner: mockRunner,
      });

      const result = await orchestrator.start(simpleConfig, { file: markdownPath });
      const clusterId = result.id;

      await waitForClusterState(orchestrator, clusterId, 'stopped', 10000);

      // Check title in ISSUE_OPENED message
      const ledger = new _Ledger(path.join(tempDir, `${clusterId}.db`));
      const messages = ledger.query({ cluster_id: clusterId, topic: 'ISSUE_OPENED' });
      assert(messages[0].content.data.title === 'my-feature');
      ledger.close();
    });
  });
}

function registerMarkdownFormattingTests() {
  describe('Markdown formatting preservation', function () {
    it('should preserve markdown headers', async function () {
      const markdownPath = path.join(tempDir, 'headers.md');
      const markdownContent = '# Main\n\n## Section\n\n### Subsection';
      fs.writeFileSync(markdownPath, markdownContent);

      mockRunner.when('worker').returns(JSON.stringify({ summary: 'Done' }));

      orchestrator = new Orchestrator({
        quiet: true,
        storageDir: tempDir,
        taskRunner: mockRunner,
      });

      const result = await orchestrator.start(simpleConfig, { file: markdownPath });
      const clusterId = result.id;

      await waitForClusterState(orchestrator, clusterId, 'stopped', 10000);

      const calls = mockRunner.getCalls('worker');
      assert(calls[0].context.includes('## Section'));
      assert(calls[0].context.includes('### Subsection'));
    });

    it('should preserve code blocks', async function () {
      const markdownPath = path.join(tempDir, 'code.md');
      const markdownContent = '# Code Example\n\n```js\nconst x = 1;\n```';
      fs.writeFileSync(markdownPath, markdownContent);

      mockRunner.when('worker').returns(JSON.stringify({ summary: 'Done' }));

      orchestrator = new Orchestrator({
        quiet: true,
        storageDir: tempDir,
        taskRunner: mockRunner,
      });

      const result = await orchestrator.start(simpleConfig, { file: markdownPath });
      const clusterId = result.id;

      await waitForClusterState(orchestrator, clusterId, 'stopped', 10000);

      const calls = mockRunner.getCalls('worker');
      assert(calls[0].context.includes('```js'));
      assert(calls[0].context.includes('const x = 1;'));
    });

    it('should preserve lists', async function () {
      const markdownPath = path.join(tempDir, 'lists.md');
      const markdownContent = '# Todo\n\n- Item 1\n- Item 2\n  - Nested';
      fs.writeFileSync(markdownPath, markdownContent);

      mockRunner.when('worker').returns(JSON.stringify({ summary: 'Done' }));

      orchestrator = new Orchestrator({
        quiet: true,
        storageDir: tempDir,
        taskRunner: mockRunner,
      });

      const result = await orchestrator.start(simpleConfig, { file: markdownPath });
      const clusterId = result.id;

      await waitForClusterState(orchestrator, clusterId, 'stopped', 10000);

      const calls = mockRunner.getCalls('worker');
      assert(calls[0].context.includes('- Item 1'));
      assert(calls[0].context.includes('- Nested'));
    });
  });
}

function registerIssueOpenedMetadataTests() {
  describe('ISSUE_OPENED message metadata', function () {
    it('should set source to "file" in metadata', async function () {
      const markdownPath = path.join(tempDir, 'test.md');
      const markdownContent = '# Test\n\nTest content.';
      fs.writeFileSync(markdownPath, markdownContent);

      mockRunner.when('worker').returns(JSON.stringify({ summary: 'Done' }));

      orchestrator = new Orchestrator({
        quiet: true,
        storageDir: tempDir,
        taskRunner: mockRunner,
      });

      const result = await orchestrator.start(simpleConfig, { file: markdownPath });
      const clusterId = result.id;

      await waitForClusterState(orchestrator, clusterId, 'stopped', 10000);

      // Check message metadata
      const ledger = new _Ledger(path.join(tempDir, `${clusterId}.db`));
      const messages = ledger.query({ cluster_id: clusterId, topic: 'ISSUE_OPENED' });
      assert.strictEqual(messages[0].metadata.source, 'file');
      ledger.close();
    });
  });
}

function registerMarkdownErrorHandlingTests() {
  describe('Error handling', function () {
    it('should throw error for nonexistent file', async function () {
      const markdownPath = path.join(tempDir, 'nonexistent.md');

      orchestrator = new Orchestrator({
        quiet: true,
        storageDir: tempDir,
        taskRunner: mockRunner,
      });

      await assert.rejects(
        () => orchestrator.start(simpleConfig, { file: markdownPath }),
        (err) => {
          return err.message.includes('File not found') && err.message.includes('nonexistent.md');
        }
      );
    });

    it('should handle relative paths', async function () {
      // Create file in temp dir
      const markdownPath = path.join(tempDir, 'relative.md');
      const markdownContent = '# Relative Path Test';
      fs.writeFileSync(markdownPath, markdownContent);

      mockRunner.when('worker').returns(JSON.stringify({ summary: 'Done' }));

      orchestrator = new Orchestrator({
        quiet: true,
        storageDir: tempDir,
        taskRunner: mockRunner,
      });

      // Use relative path (from temp dir)
      const originalCwd = process.cwd();
      try {
        process.chdir(tempDir);
        const result = await orchestrator.start(simpleConfig, { file: './relative.md' });
        const clusterId = result.id;

        await waitForClusterState(orchestrator, clusterId, 'stopped', 10000);

        mockRunner.assertCalled('worker', 1);
      } finally {
        process.chdir(originalCwd);
      }
    });

    it('should handle empty markdown file', async function () {
      const markdownPath = path.join(tempDir, 'empty.md');
      fs.writeFileSync(markdownPath, '');

      mockRunner.when('worker').returns(JSON.stringify({ summary: 'Done' }));

      orchestrator = new Orchestrator({
        quiet: true,
        storageDir: tempDir,
        taskRunner: mockRunner,
      });

      const result = await orchestrator.start(simpleConfig, { file: markdownPath });
      const clusterId = result.id;

      await waitForClusterState(orchestrator, clusterId, 'stopped', 10000);

      // Should still work, just with empty content
      mockRunner.assertCalled('worker', 1);
    });
  });
}

describe('Markdown File Input Integration', function () {
  this.timeout(30000);

  beforeEach(() => {
    tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-markdown-test-'));
    mockRunner = new MockTaskRunner();
  });

  afterEach(async () => {
    if (orchestrator) {
      const clusters = orchestrator.listClusters();
      for (const cluster of clusters) {
        try {
          await orchestrator.kill(cluster.id);
        } catch {
          // Ignore cleanup errors
        }
      }
    }
    if (tempDir && fs.existsSync(tempDir)) {
      fs.rmSync(tempDir, { recursive: true, force: true });
    }
  });

  registerBasicMarkdownInputTests();
  registerMarkdownFormattingTests();
  registerIssueOpenedMetadataTests();
  registerMarkdownErrorHandlingTests();
});
