/**
 * Regression test for issue #452:
 * `zeroshot run <issue> --pr` created a PR and immediately auto-merged it,
 * instead of stopping at PR creation for human review.
 *
 * The fix threads a single `autoMerge` boolean end-to-end:
 *   --ship (or repo settings github.autoMerge=true) -> autoMerge=true -> create + merge
 *   --pr alone                                       -> autoMerge=false -> create + STOP
 */
const assert = require('node:assert');
const os = require('node:os');
const fs = require('node:fs');
const path = require('node:path');

const { generateGitPusherAgent } = require('../src/agents/git-pusher-template');
const Orchestrator = require('../src/orchestrator');

function withTmpCwd(fn) {
  // Avoid picking up repo settings (.zeroshot/settings.json) from this repo's own cwd.
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-git-pusher-pr-mode-'));
  try {
    return fn(dir);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

describe('git-pusher --pr vs --ship (autoMerge)', function () {
  it('review mode (autoMerge=false): prompt has no merge step, output template is merged:false', function () {
    withTmpCwd((cwd) => {
      const agentConfig = generateGitPusherAgent('github', { autoMerge: false, cwd });

      assert.strictEqual(agentConfig.hooks.onComplete.config.autoMerge, false);
      assert(!/gh pr merge/i.test(agentConfig.prompt), 'prompt must not instruct gh pr merge');
      assert(
        !/MERGE THE PR \(MANDATORY/i.test(agentConfig.prompt),
        'prompt must not mandate merging the PR'
      );
      assert(
        /Do NOT merge the PR/i.test(agentConfig.prompt),
        'prompt must explicitly say do not merge'
      );
      assert(
        agentConfig.prompt.includes('"merged": false'),
        'final output template must report merged:false'
      );
      assert(
        !agentConfig.prompt.includes('"merged": true'),
        'review-mode prompt must never claim merged:true'
      );
    });
  });

  it('ship mode (autoMerge=true): prompt still merges, config carries autoMerge=true', function () {
    withTmpCwd((cwd) => {
      const agentConfig = generateGitPusherAgent('github', { autoMerge: true, cwd });

      assert.strictEqual(agentConfig.hooks.onComplete.config.autoMerge, true);
      assert(/gh pr merge/i.test(agentConfig.prompt), 'ship-mode prompt must merge the PR');
      assert(
        /MERGE THE PR \(MANDATORY/i.test(agentConfig.prompt),
        'ship-mode prompt must mandate merging'
      );
      assert(
        agentConfig.prompt.includes('"merged": true'),
        'ship-mode final output template must report merged:true'
      );
    });
  });

  it('default (no autoMerge option, no repo setting) behaves as review mode', function () {
    withTmpCwd((cwd) => {
      const agentConfig = generateGitPusherAgent('github', { cwd });
      assert.strictEqual(agentConfig.hooks.onComplete.config.autoMerge, false);
      assert(!/gh pr merge/i.test(agentConfig.prompt));
    });
  });

  it('buildPrOptions persists autoMerge=false for --pr and autoMerge=true for --ship (resume-safe)', function () {
    const prOptionsForPr = Orchestrator.buildPrOptions({ ship: false }, []);
    const prOptionsForShip = Orchestrator.buildPrOptions({ ship: true }, []);

    assert.strictEqual(prOptionsForPr.autoMerge, false);
    assert.strictEqual(prOptionsForShip.autoMerge, true);

    // Persisted even when no other PR fields were passed (--pr with all defaults),
    // otherwise the distinction is lost on `zeroshot resume`.
    assert.notStrictEqual(prOptionsForPr, null);
  });
});
