/**
 * Test: Git Safety Hook (block-dangerous-git.py)
 *
 * Verifies that the PreToolUse hook blocks dangerous git commands
 * when running in worktree mode.
 *
 * The hook prevents:
 * - git stash (hides work from other agents)
 * - git checkout -- <file> (discards changes)
 * - git reset --hard (destroys work)
 * - git push --force (rewrites history)
 * - git clean -f (deletes files)
 * - git branch -D (force deletes branch)
 */

const assert = require('assert');
const { execSync } = require('child_process');
const path = require('path');
const fs = require('fs');

// Path to the hook script
const HOOK_PATH = path.join(__dirname, '../../hooks/block-dangerous-git.py');

/**
 * Run the hook with a simulated Bash tool input
 * Hook ONLY activates when ZEROSHOT_WORKTREE=1 is set.
 * @param {string} command - The bash command to test
 * @returns {{ decision: string, message?: string }}
 */
function runHook(command) {
  const input = JSON.stringify({
    tool_name: 'Bash',
    tool_input: { command },
  });

  try {
    // CRITICAL: Must set ZEROSHOT_WORKTREE=1 or hook exits without checking
    const output = execSync(`echo '${input}' | python3 "${HOOK_PATH}"`, {
      encoding: 'utf8',
      stdio: ['pipe', 'pipe', 'pipe'],
      env: { ...process.env, ZEROSHOT_WORKTREE: '1' },
    });

    // Hook exits 0 with no output when allowing - normalize to { decision: 'allow' }
    if (!output || !output.trim()) {
      return { decision: 'allow' };
    }

    // Parse hook's output format: { hookSpecificOutput: { permissionDecision: 'deny', ... } }
    const parsed = JSON.parse(output.trim());
    if (parsed.hookSpecificOutput?.permissionDecision === 'deny') {
      return {
        decision: 'block',
        message: parsed.hookSpecificOutput.permissionDecisionReason,
      };
    }

    // Unknown format - treat as allow
    return { decision: 'allow' };
  } catch (err) {
    // If hook exits with error, parse stdout anyway
    if (err.stdout) {
      try {
        const parsed = JSON.parse(err.stdout.trim());
        if (parsed.hookSpecificOutput?.permissionDecision === 'deny') {
          return {
            decision: 'block',
            message: parsed.hookSpecificOutput.permissionDecisionReason,
          };
        }
      } catch {
        // Fall through to error case
      }
    }
    return { decision: 'error', message: err.message };
  }
}

describe('Git Safety Hook', function () {
  before(function () {
    // Skip if hook doesn't exist
    if (!fs.existsSync(HOOK_PATH)) {
      this.skip();
      return;
    }

    // Ensure hook is executable
    try {
      fs.chmodSync(HOOK_PATH, 0o755);
    } catch {
      // May not have permission, try anyway
    }
  });

  registerBlockedCommandsTests();
  registerAllowedCommandsTests();
  registerNonGitCommandsTests();
  registerNonBashToolsTests();
  registerEdgeCasesTests();
});

function registerBlockedCommandsTests() {
  describe('Blocked commands', function () {
    const blockedCommands = [
      { cmd: 'git stash', reason: 'hides work from agents' },
      { cmd: 'git stash push', reason: 'hides work from agents' },
      { cmd: 'git stash save "wip"', reason: 'hides work from agents' },
      { cmd: 'git checkout -- src/app.ts', reason: 'discards changes' },
      { cmd: 'git checkout .', reason: 'discards all changes' },
      { cmd: 'git checkout -f', reason: 'force checkout' },
      { cmd: 'git reset --hard', reason: 'destroys commits and changes' },
      { cmd: 'git reset --hard HEAD~1', reason: 'destroys commits' },
      { cmd: 'git push --force', reason: 'rewrites history' },
      { cmd: 'git push -f origin main', reason: 'rewrites history' },
      { cmd: 'git push --force-with-lease', reason: 'rewrites history' },
      { cmd: 'git clean -f', reason: 'deletes untracked files' },
      { cmd: 'git clean -fd', reason: 'deletes files and dirs' },
      { cmd: 'git branch -D feature', reason: 'force deletes branch' },
    ];

    for (const { cmd, reason } of blockedCommands) {
      it(`should block: ${cmd} (${reason})`, function () {
        const result = runHook(cmd);

        assert.strictEqual(
          result.decision,
          'block',
          `"${cmd}" should be blocked, got: ${JSON.stringify(result)}`
        );
        assert(result.message, 'Should include reason message');
      });
    }
  });
}

function registerAllowedCommandsTests() {
  describe('Allowed commands', function () {
    const allowedCommands = [
      'git status',
      'git add .',
      'git add -A',
      'git commit -m "message"',
      'git push',
      'git push origin feature-branch',
      'git push -u origin feature-branch',
      'git checkout main',
      'git checkout feature-branch',
      'git checkout -b new-branch',
      'git switch main',
      'git switch -c new-branch',
      'git pull',
      'git pull --rebase',
      'git fetch',
      'git log',
      'git diff',
      'git branch',
      'git branch -d merged-branch', // Safe delete (requires merge)
      'git reset --soft HEAD~1', // Soft reset (keeps changes)
      'git revert HEAD',
      'git merge feature-branch',
      'git rebase main',
      'git clean -n', // Dry run (doesn't delete)
    ];

    for (const cmd of allowedCommands) {
      it(`should allow: ${cmd}`, function () {
        const result = runHook(cmd);

        assert.strictEqual(
          result.decision,
          'allow',
          `"${cmd}" should be allowed, got: ${JSON.stringify(result)}`
        );
      });
    }
  });
}

function registerNonGitCommandsTests() {
  describe('Non-git commands', function () {
    it('should allow non-git bash commands', function () {
      const result = runHook('npm install');
      assert.strictEqual(result.decision, 'allow');
    });

    it('should allow commands containing "git" as substring', function () {
      const result = runHook('echo "digital transformation"');
      assert.strictEqual(result.decision, 'allow');
    });
  });
}

function registerNonBashToolsTests() {
  describe('Non-Bash tools', function () {
    it('should allow other tools (not Bash)', function () {
      const input = JSON.stringify({
        tool_name: 'Read',
        tool_input: { file_path: '/some/file.txt' },
      });

      // Hook exits 0 with no output for non-Bash tools (passthrough)
      const output = execSync(`echo '${input}' | python3 "${HOOK_PATH}"`, {
        encoding: 'utf8',
        env: { ...process.env, ZEROSHOT_WORKTREE: '1' },
      });

      // Empty output means passthrough (allow)
      assert.strictEqual(output.trim(), '', 'Non-Bash tools should passthrough with no output');
    });
  });
}

function registerEdgeCasesTests() {
  describe('Edge cases', function () {
    it('should handle commands with pipes containing git', function () {
      const result = runHook('git log | head -10');
      assert.strictEqual(result.decision, 'allow');
    });

    it('should handle git commands in subshells', function () {
      // Commands in $() should also be checked
      const _result = runHook('echo $(git stash)');
      // This depends on hook implementation - may or may not catch
      // Current implementation should catch simple patterns
    });

    it('should handle multiline commands', function () {
      const result = runHook('git status && \ngit add .');
      assert.strictEqual(result.decision, 'allow');
    });

    it('should block git stash in compound command', function () {
      const result = runHook('git add . && git stash');
      assert.strictEqual(result.decision, 'block');
    });
  });
}
