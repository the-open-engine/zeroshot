const assert = require('assert');
const { listProviders, getProvider } = require('../../src/providers');
const { normalizeProviderName, VALID_PROVIDERS } = require('../../lib/provider-names');
const { CAPABILITIES } = require('../../src/providers/capabilities');
const { buildCommand } = require('../../src/providers/copilot/cli-builder');
const { parseEvent, parseChunk } = require('../../src/providers/copilot/output-parser');
const CopilotProvider = require('../../src/providers/copilot');

describe('Copilot provider', function () {
  it('is registered in the provider list', function () {
    assert.ok(listProviders().includes('copilot'));
    assert.ok(VALID_PROVIDERS.includes('copilot'));
  });

  it('getProvider("copilot") returns a CopilotProvider', function () {
    const provider = getProvider('copilot');
    assert.ok(provider instanceof CopilotProvider);
    assert.strictEqual(provider.name, 'copilot');
    assert.strictEqual(provider.displayName, 'Copilot');
    assert.strictEqual(provider.cliCommand, 'copilot');
  });

  it('normalizes the "github" alias to copilot', function () {
    assert.strictEqual(normalizeProviderName('github'), 'copilot');
    assert.strictEqual(normalizeProviderName('copilot'), 'copilot');
    assert.strictEqual(normalizeProviderName('Copilot'), 'copilot');
  });

  it('exposes capability flags', function () {
    const caps = CAPABILITIES.copilot;
    assert.ok(caps);
    assert.strictEqual(caps.dockerIsolation, true);
    assert.strictEqual(caps.worktreeIsolation, true);
    assert.strictEqual(caps.mcpServers, true);
    assert.strictEqual(caps.streamJson, false);
    assert.strictEqual(caps.thinkingMode, false);
    assert.strictEqual(caps.reasoningEffort, false);
    assert.strictEqual(caps.jsonSchema, 'experimental');
  });

  it('install/auth instructions reference @github/copilot and /login', function () {
    const provider = new CopilotProvider();
    assert.match(provider.getInstallInstructions(), /@github\/copilot/);
    assert.match(provider.getAuthInstructions(), /\/login/);
    assert.deepStrictEqual(provider.getCredentialPaths(), ['~/.copilot']);
  });

  describe('buildCommand', function () {
    it('passes the prompt as the value of -p and includes --silent', function () {
      const result = buildCommand('hello world', {
        autoApprove: true,
        cliFeatures: { supportsModel: true, supportsAllowAll: true, supportsSilent: true },
      });

      assert.strictEqual(result.binary, 'copilot');
      const pIndex = result.args.indexOf('-p');
      assert.notStrictEqual(pIndex, -1);
      assert.strictEqual(result.args[pIndex + 1], 'hello world');
      assert.ok(result.args.includes('--silent'));
      assert.ok(result.args.includes('--allow-all'));
    });

    it('omits --allow-all when autoApprove is false', function () {
      const result = buildCommand('prompt', {
        autoApprove: false,
        cliFeatures: { supportsAllowAll: true },
      });
      assert.ok(!result.args.includes('--allow-all'));
    });

    it('includes --model X when modelSpec.model is set', function () {
      const result = buildCommand('prompt', {
        modelSpec: { model: 'claude-sonnet-4.5' },
        cliFeatures: { supportsModel: true, supportsAllowAll: true, supportsSilent: true },
      });
      const idx = result.args.indexOf('--model');
      assert.notStrictEqual(idx, -1);
      assert.strictEqual(result.args[idx + 1], 'claude-sonnet-4.5');
    });

    it('skips --model when feature flag says unsupported', function () {
      const result = buildCommand('prompt', {
        modelSpec: { model: 'claude-sonnet-4.5' },
        cliFeatures: { supportsModel: false },
      });
      assert.ok(!result.args.includes('--model'));
    });

    it('injects jsonSchema into the prompt as OUTPUT FORMAT block', function () {
      const result = buildCommand('do a thing', {
        jsonSchema: { type: 'object', properties: { ok: { type: 'boolean' } } },
        cliFeatures: {},
      });
      const pIndex = result.args.indexOf('-p');
      const ctx = result.args[pIndex + 1];
      assert.match(ctx, /## OUTPUT FORMAT/);
      assert.match(ctx, /"ok"/);
    });

    it('passes mcpConfig as JSON string via --additional-mcp-config', function () {
      const result = buildCommand('p', {
        mcpConfig: '{"mcpServers":{"x":{"command":"true"}}}',
        cliFeatures: { supportsMcpConfig: true },
      });
      const i = result.args.indexOf('--additional-mcp-config');
      assert.notStrictEqual(i, -1);
      assert.strictEqual(result.args[i + 1], '{"mcpServers":{"x":{"command":"true"}}}');
    });

    it('serializes object mcpConfig to JSON', function () {
      const cfg = { mcpServers: { fs: { command: 'npx', args: ['-y', 'pkg'] } } };
      const result = buildCommand('p', {
        mcpConfig: cfg,
        cliFeatures: { supportsMcpConfig: true },
      });
      const i = result.args.indexOf('--additional-mcp-config');
      assert.strictEqual(result.args[i + 1], JSON.stringify(cfg));
    });

    it('emits --additional-mcp-config once per array entry, supports @file paths', function () {
      const result = buildCommand('p', {
        mcpConfig: ['@./mcp-a.json', { mcpServers: { b: { command: 'b' } } }],
        cliFeatures: { supportsMcpConfig: true },
      });
      const flags = result.args.filter((a) => a === '--additional-mcp-config');
      assert.strictEqual(flags.length, 2);
      assert.ok(result.args.includes('@./mcp-a.json'));
      assert.ok(result.args.includes(JSON.stringify({ mcpServers: { b: { command: 'b' } } })));
    });

    it('omits --additional-mcp-config when CLI does not support it', function () {
      const result = buildCommand('p', {
        mcpConfig: '{}',
        cliFeatures: { supportsMcpConfig: false },
      });
      assert.ok(!result.args.includes('--additional-mcp-config'));
    });

    it('passes addDirs as repeated --add-dir flags', function () {
      const result = buildCommand('p', {
        addDirs: ['/tmp/a', '/tmp/b'],
        cliFeatures: { supportsAddDir: true },
      });
      const flags = result.args.filter((a) => a === '--add-dir');
      assert.strictEqual(flags.length, 2);
      assert.ok(result.args.includes('/tmp/a'));
      assert.ok(result.args.includes('/tmp/b'));
    });
  });

  describe('parseEvent / parseChunk', function () {
    it('returns text event for a non-empty plain line', function () {
      assert.deepStrictEqual(parseEvent('hello world'), { type: 'text', text: 'hello world' });
    });

    it('returns null for empty/whitespace lines', function () {
      assert.strictEqual(parseEvent(''), null);
      assert.strictEqual(parseEvent('   '), null);
      assert.strictEqual(parseEvent(null), null);
    });

    it('parses structured JSON text events when present', function () {
      const ev = parseEvent('{"type":"text","text":"hi"}');
      assert.deepStrictEqual(ev, { type: 'text', text: 'hi' });
    });

    it('parses structured JSON error events as failed result', function () {
      const ev = parseEvent('{"type":"error","error":"boom"}');
      assert.strictEqual(ev.type, 'result');
      assert.strictEqual(ev.success, false);
      assert.strictEqual(ev.error, 'boom');
    });

    it('parseChunk splits multiple lines into events', function () {
      const events = parseChunk('first line\nsecond line\n');
      assert.strictEqual(events.length, 2);
      assert.strictEqual(events[0].text, 'first line');
      assert.strictEqual(events[1].text, 'second line');
    });
  });

  describe('levels and models', function () {
    const provider = new CopilotProvider();

    it('exposes the expected level mapping', function () {
      const mapping = provider.getLevelMapping();
      assert.strictEqual(mapping.level1.model, 'gpt-5-mini');
      assert.strictEqual(mapping.level2.model, 'claude-sonnet-4.5');
      assert.strictEqual(mapping.level3.model, 'claude-opus-4.6');
    });

    it('uses sensible defaults', function () {
      assert.strictEqual(provider.getDefaultLevel(), 'level2');
      assert.strictEqual(provider.getDefaultMinLevel(), 'level1');
      assert.strictEqual(provider.getDefaultMaxLevel(), 'level3');
    });

    it('validateLevel accepts in-range, rejects out-of-range', function () {
      assert.strictEqual(provider.validateLevel('level2', 'level1', 'level3'), 'level2');
      assert.throws(() => provider.validateLevel('level9', 'level1', 'level3'), /Invalid level/);
    });

    it('resolveModelSpec returns the level model', function () {
      const spec = provider.resolveModelSpec('level3', {});
      assert.strictEqual(spec.model, 'claude-opus-4.6');
    });

    it('catalog contains expected models', function () {
      const catalog = provider.getModelCatalog();
      assert.ok(catalog['gpt-5-mini']);
      assert.ok(catalog['claude-sonnet-4.5']);
      assert.ok(catalog['claude-opus-4.6']);
      assert.ok(catalog['gpt-5']);
    });
  });
});
