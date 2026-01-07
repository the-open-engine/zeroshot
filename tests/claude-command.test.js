/**
 * Tests for getClaudeCommand helper
 */
const { expect } = require('chai');
const fs = require('fs');
const path = require('path');
const os = require('os');

describe('getClaudeCommand', () => {
  let originalEnv;
  let settingsFile;

  beforeEach(() => {
    // Save original env
    originalEnv = { ...process.env };
    // Use temp settings file
    settingsFile = path.join(os.tmpdir(), `test-settings-${Date.now()}.json`);
    process.env.ZEROSHOT_SETTINGS_FILE = settingsFile;
    delete process.env.ZEROSHOT_CLAUDE_COMMAND;
  });

  afterEach(() => {
    // Restore original env
    process.env = originalEnv;
    // Cleanup temp file
    if (fs.existsSync(settingsFile)) {
      fs.unlinkSync(settingsFile);
    }
  });

  it('returns default claude command when no settings', () => {
    // Clear cache and reimport
    delete require.cache[require.resolve('../lib/settings.js')];
    const { getClaudeCommand } = require('../lib/settings.js');

    const result = getClaudeCommand();
    expect(result.command).to.equal('claude');
    expect(result.args).to.deep.equal([]);
  });

  it('returns configured command from settings', () => {
    fs.writeFileSync(settingsFile, JSON.stringify({ claudeCommand: 'ccr code' }));

    delete require.cache[require.resolve('../lib/settings.js')];
    const { getClaudeCommand } = require('../lib/settings.js');

    const result = getClaudeCommand();
    expect(result.command).to.equal('ccr');
    expect(result.args).to.deep.equal(['code']);
  });

  it('parses multi-arg commands correctly', () => {
    fs.writeFileSync(settingsFile, JSON.stringify({ claudeCommand: 'my-cmd arg1 arg2 arg3' }));

    delete require.cache[require.resolve('../lib/settings.js')];
    const { getClaudeCommand } = require('../lib/settings.js');

    const result = getClaudeCommand();
    expect(result.command).to.equal('my-cmd');
    expect(result.args).to.deep.equal(['arg1', 'arg2', 'arg3']);
  });

  it('environment variable overrides settings', () => {
    fs.writeFileSync(settingsFile, JSON.stringify({ claudeCommand: 'from-settings' }));
    process.env.ZEROSHOT_CLAUDE_COMMAND = 'from-env with args';

    delete require.cache[require.resolve('../lib/settings.js')];
    const { getClaudeCommand } = require('../lib/settings.js');

    const result = getClaudeCommand();
    expect(result.command).to.equal('from-env');
    expect(result.args).to.deep.equal(['with', 'args']);
  });

  it('handles whitespace in command correctly', () => {
    fs.writeFileSync(settingsFile, JSON.stringify({ claudeCommand: '  trimmed   cmd  ' }));

    delete require.cache[require.resolve('../lib/settings.js')];
    const { getClaudeCommand } = require('../lib/settings.js');

    const result = getClaudeCommand();
    expect(result.command).to.equal('trimmed');
    expect(result.args).to.deep.equal(['cmd']);
  });
});

describe('claudeCommand setting validation', () => {
  it('rejects empty string', () => {
    const { validateSetting } = require('../lib/settings.js');
    const error = validateSetting('claudeCommand', '');
    expect(error).to.equal('claudeCommand cannot be empty');
  });

  it('rejects whitespace-only string', () => {
    const { validateSetting } = require('../lib/settings.js');
    const error = validateSetting('claudeCommand', '   ');
    expect(error).to.equal('claudeCommand cannot be empty');
  });

  it('accepts valid command string', () => {
    const { validateSetting } = require('../lib/settings.js');
    const error = validateSetting('claudeCommand', 'ccr code');
    expect(error).to.be.null;
  });

  it('rejects non-string value', () => {
    const { validateSetting } = require('../lib/settings.js');
    const error = validateSetting('claudeCommand', 123);
    expect(error).to.equal('claudeCommand must be a string');
  });
});
