/**
 * Test: stdin task input ('zeroshot run -')
 *
 * Ensures piped/redirected task bodies are preserved verbatim, avoiding
 * shell-quoting breakage from inline args.
 */

const assert = require('assert');
const { Readable } = require('stream');
const {
  isStdinInput,
  readStdinText,
  encodeStdinEnv,
  decodeStdinEnv,
  buildTextInput,
} = require('../../lib/start-cluster');

describe('CLI stdin task input', function () {
  describe('isStdinInput', function () {
    it('recognizes the "-" marker', function () {
      assert.strictEqual(isStdinInput('-'), true);
    });

    it('rejects issue numbers, files, and plain text', function () {
      assert.strictEqual(isStdinInput('123'), false);
      assert.strictEqual(isStdinInput('feature.md'), false);
      assert.strictEqual(isStdinInput('some text'), false);
    });
  });

  describe('readStdinText', function () {
    it('collects and trims piped chunks', async function () {
      const stream = Readable.from(['line1\n', 'line2\n']);
      const text = await readStdinText(stream);
      assert.strictEqual(text, 'line1\nline2');
    });

    it('resolves to empty string for empty stdin', async function () {
      const stream = Readable.from([]);
      const text = await readStdinText(stream);
      assert.strictEqual(text, '');
    });
  });

  describe('encodeStdinEnv / decodeStdinEnv', function () {
    it('round-trips text containing shell metacharacters and unicode', function () {
      const text = 'BrandContext.writingStyle `backtick` $(command) über\nmultiline';
      const encoded = encodeStdinEnv(text);
      const decoded = decodeStdinEnv(encoded);
      assert.strictEqual(decoded, text);
    });

    it('produces unmodified text input after round-trip', function () {
      const text = 'echo `whoami` $(rm -rf /)';
      const result = buildTextInput(decodeStdinEnv(encodeStdinEnv(text)));
      assert.deepStrictEqual(result, { text });
    });
  });
});
