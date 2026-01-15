/**
 * Test: InputHelpers.createFileInput()
 *
 * Verifies markdown file reading and parsing logic
 * Tests: file reading, title extraction, path resolution, error handling
 */

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const os = require('os');
const InputHelpers = require('../../src/input-helpers');

let tempDir;

describe('InputHelpers.createFileInput()', function () {
  beforeEach(function () {
    // Create temp directory for test files
    tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-test-'));
  });

  afterEach(function () {
    // Clean up temp directory
    if (fs.existsSync(tempDir)) {
      fs.rmSync(tempDir, { recursive: true, force: true });
    }
  });

  registerFileReadingTests();
  registerTitleExtractionTests();
  registerOutputStructureTests();
  registerMarkdownFormattingTests();
  registerEdgeCaseTests();
});

function registerFileReadingTests() {
  describe('File reading', function () {
    it('should read file content correctly', function () {
      const filePath = path.join(tempDir, 'test.md');
      const content = '# Test Feature\n\nThis is a test.';
      fs.writeFileSync(filePath, content);

      const result = InputHelpers.createFileInput(filePath);

      assert.strictEqual(result.body, content);
      assert.ok(result.context.includes(content));
    });

    it('should throw ENOENT error for missing file', function () {
      const filePath = path.join(tempDir, 'nonexistent.md');

      assert.throws(
        () => InputHelpers.createFileInput(filePath),
        (err) => {
          return err.message.includes('File not found') && err.message.includes('nonexistent.md');
        }
      );
    });

    it('should resolve relative paths', function () {
      const fileName = 'relative.md';
      const content = '# Relative Path Test';

      // Write file in current directory
      fs.writeFileSync(fileName, content);

      try {
        const result = InputHelpers.createFileInput(fileName);
        assert.strictEqual(result.body, content);
      } finally {
        // Clean up
        if (fs.existsSync(fileName)) {
          fs.unlinkSync(fileName);
        }
      }
    });

    it('should handle absolute paths', function () {
      const filePath = path.join(tempDir, 'absolute.md');
      const content = '# Absolute Path Test';
      fs.writeFileSync(filePath, content);

      const result = InputHelpers.createFileInput(filePath);

      assert.strictEqual(result.body, content);
    });
  });
}

function registerTitleExtractionTests() {
  describe('Title extraction', function () {
    it('should extract title from first # header', function () {
      const filePath = path.join(tempDir, 'test.md');
      const content = '# Feature Title\n\nDescription here.';
      fs.writeFileSync(filePath, content);

      const result = InputHelpers.createFileInput(filePath);

      assert.strictEqual(result.title, 'Feature Title');
    });

    it('should trim whitespace from extracted title', function () {
      const filePath = path.join(tempDir, 'test.md');
      const content = '#   Spaced Title   \n\nDescription.';
      fs.writeFileSync(filePath, content);

      const result = InputHelpers.createFileInput(filePath);

      assert.strictEqual(result.title, 'Spaced Title');
    });

    it('should fall back to filename if no header', function () {
      const filePath = path.join(tempDir, 'my-feature.md');
      const content = 'No header here, just text.';
      fs.writeFileSync(filePath, content);

      const result = InputHelpers.createFileInput(filePath);

      assert.strictEqual(result.title, 'my-feature');
    });

    it('should ignore ## headers (only # counts)', function () {
      const filePath = path.join(tempDir, 'test.md');
      const content = '## Second Level\n\nThis should use filename.';
      fs.writeFileSync(filePath, content);

      const result = InputHelpers.createFileInput(filePath);

      assert.strictEqual(result.title, 'test');
    });

    it('should extract first # header even with content before it', function () {
      const filePath = path.join(tempDir, 'test.md');
      const content = 'Some intro text\n\n# Main Title\n\nMore text.';
      fs.writeFileSync(filePath, content);

      const result = InputHelpers.createFileInput(filePath);

      assert.strictEqual(result.title, 'Main Title');
    });
  });
}

function registerOutputStructureTests() {
  describe('Output structure', function () {
    it('should match _parseIssue format', function () {
      const filePath = path.join(tempDir, 'test.md');
      const content = '# Test\n\nContent here.';
      fs.writeFileSync(filePath, content);

      const result = InputHelpers.createFileInput(filePath);

      assert.strictEqual(result.number, null);
      assert.strictEqual(typeof result.title, 'string');
      assert.strictEqual(result.body, content);
      assert.ok(Array.isArray(result.labels));
      assert.strictEqual(result.labels.length, 0);
      assert.ok(Array.isArray(result.comments));
      assert.strictEqual(result.comments.length, 0);
      assert.strictEqual(result.url, null);
      assert.ok(typeof result.context === 'string');
    });

    it('should include title and body in context', function () {
      const filePath = path.join(tempDir, 'test.md');
      const content = '# Feature Request\n\nAdd dark mode.';
      fs.writeFileSync(filePath, content);

      const result = InputHelpers.createFileInput(filePath);

      assert.ok(result.context.includes('Feature Request'));
      assert.ok(result.context.includes('Add dark mode.'));
    });
  });
}

function registerMarkdownFormattingTests() {
  describe('Markdown formatting preservation', function () {
    it('should preserve headers', function () {
      const filePath = path.join(tempDir, 'test.md');
      const content = '# Main\n\n## Section\n\n### Subsection';
      fs.writeFileSync(filePath, content);

      const result = InputHelpers.createFileInput(filePath);

      assert.strictEqual(result.body, content);
    });

    it('should preserve lists', function () {
      const filePath = path.join(tempDir, 'test.md');
      const content = '# List Test\n\n- Item 1\n- Item 2\n  - Nested\n';
      fs.writeFileSync(filePath, content);

      const result = InputHelpers.createFileInput(filePath);

      assert.strictEqual(result.body, content);
    });

    it('should preserve code blocks', function () {
      const filePath = path.join(tempDir, 'test.md');
      const content = '# Code Test\n\n```js\nconst x = 1;\n```';
      fs.writeFileSync(filePath, content);

      const result = InputHelpers.createFileInput(filePath);

      assert.strictEqual(result.body, content);
      assert.ok(result.body.includes('```js'));
    });

    it('should preserve inline code', function () {
      const filePath = path.join(tempDir, 'test.md');
      const content = '# Test\n\nUse `npm install` to install.';
      fs.writeFileSync(filePath, content);

      const result = InputHelpers.createFileInput(filePath);

      assert.strictEqual(result.body, content);
      assert.ok(result.body.includes('`npm install`'));
    });
  });
}

function registerEdgeCaseTests() {
  describe('Edge cases', function () {
    it('should handle empty file', function () {
      const filePath = path.join(tempDir, 'empty.md');
      fs.writeFileSync(filePath, '');

      const result = InputHelpers.createFileInput(filePath);

      assert.strictEqual(result.body, '');
      assert.strictEqual(result.title, 'empty');
    });

    it('should handle very large file', function () {
      const filePath = path.join(tempDir, 'large.md');
      const content = '# Large File\n\n' + 'x'.repeat(200000); // 200KB
      fs.writeFileSync(filePath, content);

      const result = InputHelpers.createFileInput(filePath);

      assert.strictEqual(result.title, 'Large File');
      assert.strictEqual(result.body.length, content.length);
    });

    it('should handle markdown with special characters in path', function () {
      const filePath = path.join(tempDir, 'file with spaces.md');
      const content = '# Special Path';
      fs.writeFileSync(filePath, content);

      const result = InputHelpers.createFileInput(filePath);

      assert.strictEqual(result.title, 'Special Path');
    });

    it('should handle .markdown extension', function () {
      const filePath = path.join(tempDir, 'test.markdown');
      const content = '# Markdown Extension';
      fs.writeFileSync(filePath, content);

      const result = InputHelpers.createFileInput(filePath);

      assert.strictEqual(result.title, 'Markdown Extension');
    });

    it('should handle file with only whitespace', function () {
      const filePath = path.join(tempDir, 'whitespace.md');
      fs.writeFileSync(filePath, '   \n\n   ');

      const result = InputHelpers.createFileInput(filePath);

      assert.strictEqual(result.body, '   \n\n   ');
      assert.strictEqual(result.title, 'whitespace');
    });
  });
}
