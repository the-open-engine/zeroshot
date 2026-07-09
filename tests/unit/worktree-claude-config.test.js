const assert = require('assert');
const fs = require('fs');
const os = require('os');
const path = require('path');
const sinon = require('sinon');

const { prepareClaudeConfigDir } = require('../../src/worktree-claude-config');
const safeExec = require('../../src/lib/safe-exec');

function withPlatform(value, fn) {
  const original = Object.getOwnPropertyDescriptor(os, 'platform');
  Object.defineProperty(os, 'platform', { value: () => value, configurable: true });
  try {
    return fn();
  } finally {
    Object.defineProperty(os, 'platform', original);
  }
}

function loadWorktreeClaudeConfigWithStubbedCredentials(execSyncStub) {
  sinon.stub(safeExec, 'execSync').callsFake(execSyncStub);
  delete require.cache[require.resolve('../../src/claude-credentials')];
  delete require.cache[require.resolve('../../src/worktree-claude-config')];
  return require('../../src/worktree-claude-config');
}

describe('worktree-claude-config', function () {
  /** @type {string[]} */
  let tempDirs = [];

  afterEach(function () {
    sinon.restore();
    delete require.cache[require.resolve('../../src/claude-credentials')];
    delete require.cache[require.resolve('../../src/worktree-claude-config')];
    for (const dir of tempDirs.splice(0)) {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  it('returns null when the worktree has no repo-owned claude config', function () {
    const worktreeRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-claude-worktree-'));
    tempDirs.push(worktreeRoot);
    fs.writeFileSync(path.join(worktreeRoot, '.git'), 'gitdir: test\n', 'utf8');

    assert.strictEqual(prepareClaudeConfigDir({ worktreePath: worktreeRoot }), null);
  });

  it('merges repo-owned settings and MCP config into a temporary overlay config dir', function () {
    const worktreeRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-claude-worktree-'));
    const sourceDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-claude-source-'));
    tempDirs.push(worktreeRoot, sourceDir);

    fs.writeFileSync(path.join(worktreeRoot, '.git'), 'gitdir: test\n', 'utf8');
    fs.mkdirSync(path.join(worktreeRoot, '.claude'), { recursive: true });
    fs.writeFileSync(
      path.join(worktreeRoot, '.claude', 'settings.json'),
      JSON.stringify(
        {
          hooks: {
            PreToolUse: [
              {
                matcher: 'Edit|Write',
                hooks: [
                  { type: 'command', command: '$CLAUDE_PROJECT_DIR/.claude/hooks/pre_tool_use.sh' },
                ],
              },
            ],
          },
        },
        null,
        2
      )
    );
    fs.writeFileSync(
      path.join(worktreeRoot, '.claude', '.mcp.json'),
      JSON.stringify(
        {
          mcpServers: {
            repo: {
              command: 'repo-tool',
              args: ['serve'],
            },
          },
        },
        null,
        2
      )
    );

    fs.writeFileSync(path.join(sourceDir, '.credentials.json'), '{"token":"secret"}\n', 'utf8');
    fs.writeFileSync(
      path.join(sourceDir, 'settings.json'),
      JSON.stringify(
        {
          hooks: {
            PreToolUse: [
              {
                matcher: 'AskUserQuestion',
                hooks: [{ type: 'command', command: '/tmp/block-ask.py' }],
              },
            ],
          },
        },
        null,
        2
      )
    );
    fs.writeFileSync(
      path.join(sourceDir, '.mcp.json'),
      JSON.stringify(
        {
          mcpServers: {
            shared: {
              command: 'npx',
              args: ['-y', '@acme/shared-mcp'],
            },
          },
        },
        null,
        2
      )
    );

    const overlayDir = prepareClaudeConfigDir({ worktreePath: worktreeRoot, sourceDir });
    tempDirs.push(overlayDir);

    assert.ok(overlayDir, 'expected an overlay config dir');
    assert.ok(fs.existsSync(path.join(overlayDir, '.credentials.json')));

    const mergedSettings = JSON.parse(
      fs.readFileSync(path.join(overlayDir, 'settings.json'), 'utf8')
    );
    assert.strictEqual(mergedSettings.hooks.PreToolUse.length, 2);
    assert.deepStrictEqual(mergedSettings.hooks.PreToolUse.map((entry) => entry.matcher).sort(), [
      'AskUserQuestion',
      'Edit|Write',
    ]);

    const mergedMcp = JSON.parse(fs.readFileSync(path.join(overlayDir, '.mcp.json'), 'utf8'));
    assert.deepStrictEqual(Object.keys(mergedMcp.mcpServers).sort(), ['repo', 'shared']);
  });

  it('materializes macOS Keychain credentials into the isolated overlay config dir', function () {
    const worktreeRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-claude-worktree-'));
    const sourceDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-claude-source-no-creds-'));
    tempDirs.push(worktreeRoot, sourceDir);

    fs.writeFileSync(path.join(worktreeRoot, '.git'), 'gitdir: test\n', 'utf8');
    fs.mkdirSync(path.join(worktreeRoot, '.claude'), { recursive: true });
    fs.writeFileSync(path.join(worktreeRoot, '.claude', 'settings.json'), '{}\n', 'utf8');

    const keychainJson = '{"claudeAiOauth":{"accessToken":"keychain-token"}}';
    const { prepareClaudeConfigDir: prepareWithStubbedKeychain } =
      loadWorktreeClaudeConfigWithStubbedCredentials((command) => {
        assert.strictEqual(
          command,
          'security find-generic-password -s "Claude Code-credentials" -w'
        );
        return `${keychainJson}\n`;
      });

    const overlayDir = withPlatform('darwin', () =>
      prepareWithStubbedKeychain({ worktreePath: worktreeRoot, sourceDir })
    );
    tempDirs.push(overlayDir);

    assert.ok(overlayDir, 'expected an overlay config dir');
    assert.strictEqual(
      fs.readFileSync(path.join(overlayDir, '.credentials.json'), 'utf8'),
      keychainJson,
      'isolated CLAUDE_CONFIG_DIR must receive materialized Keychain credentials'
    );
    assert.strictEqual(
      fs.statSync(path.join(overlayDir, '.credentials.json')).mode & 0o777,
      0o600,
      'materialized credentials should be owner-readable only'
    );
  });
});
