const assert = require('assert');
const fs = require('fs');
const os = require('os');
const path = require('path');
const Orchestrator = require('../src/orchestrator');

describe('Orchestrator completion-detector injection', function () {
  const originalSettingsFile = process.env.ZEROSHOT_SETTINGS_FILE;
  let tempDir;

  afterEach(function () {
    if (tempDir && fs.existsSync(tempDir)) {
      fs.rmSync(tempDir, { recursive: true, force: true });
      tempDir = null;
    }

    if (originalSettingsFile) {
      process.env.ZEROSHOT_SETTINGS_FILE = originalSettingsFile;
    } else {
      delete process.env.ZEROSHOT_SETTINGS_FILE;
    }
  });

  function writeSettings(settings) {
    tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-completion-detector-'));
    const settingsFile = path.join(tempDir, 'settings.json');
    fs.writeFileSync(settingsFile, JSON.stringify(settings), 'utf8');
    process.env.ZEROSHOT_SETTINGS_FILE = settingsFile;
  }

  it('uses modelLevel derived from claude minModel bounds', async function () {
    writeSettings({
      maxModel: 'opus',
      minModel: 'sonnet',
    });

    const orchestrator = new Orchestrator({ quiet: true, skipLoad: true });
    let injectedAgent = null;
    orchestrator._opAddAgents = (_cluster, operation) => {
      injectedAgent = operation.agents[0];
    };

    await orchestrator._injectCompletionAgent(
      {
        agents: [],
        config: { defaultProvider: 'claude' },
        autoPr: false,
      },
      {}
    );

    assert.ok(injectedAgent, 'Expected completion-detector to be injected');
    assert.strictEqual(injectedAgent.id, 'completion-detector');
    assert.strictEqual(injectedAgent.modelLevel, 'level2');
    assert.strictEqual(injectedAgent.model, undefined);
  });

  it('uses provider-specific minLevel when forcing non-claude provider', async function () {
    writeSettings({
      defaultProvider: 'codex',
      providerSettings: {
        codex: {
          minLevel: 'level3',
        },
      },
    });

    const orchestrator = new Orchestrator({ quiet: true, skipLoad: true });
    let injectedAgent = null;
    orchestrator._opAddAgents = (_cluster, operation) => {
      injectedAgent = operation.agents[0];
    };

    await orchestrator._injectCompletionAgent(
      {
        agents: [],
        config: { forceProvider: 'codex' },
        autoPr: false,
      },
      {}
    );

    assert.ok(injectedAgent, 'Expected completion-detector to be injected');
    assert.strictEqual(injectedAgent.modelLevel, 'level3');
    assert.strictEqual(injectedAgent.model, undefined);
  });
});
