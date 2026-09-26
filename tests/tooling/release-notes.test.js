'use strict';

const assert = require('node:assert/strict');
const childProcess = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { describe, it } = require('node:test');
const {
  generateReleaseNotes,
  parseReleaseCommit,
  renderReleaseNotes,
  summaryFromBody,
} = require('../../scripts/release-notes');

// Git hooks export repository-local variables. A fixture must not inherit the parent
// worktree's Git directory or index, including when release-note reads spawn Git.
const localGitVariables = childProcess
  .execFileSync('git', ['rev-parse', '--local-env-vars'], { encoding: 'utf8' })
  .trim()
  .split('\n');
for (const name of localGitVariables) delete process.env[name];

const commit = (hash, subject, body) => ({ hash: hash.repeat(40), subject, body });
const git = (repository, ...arguments_) =>
  childProcess.execFileSync('git', arguments_, { cwd: repository, encoding: 'utf8' }).trim();

describe('deterministic release notes', () => {
  it('extracts the Summary section without validation prose', () => {
    const body = [
      '## Summary',
      '',
      'Add the local profile editor.',
      '',
      '- Preserve saved revisions.',
      '',
      '## Testing',
      '',
      '- npm test',
    ].join('\n');
    assert.equal(
      summaryFromBody(body),
      'Add the local profile editor.\n\n- Preserve saved revisions.'
    );
  });

  it('rejects leading prose when a squash body has no Summary heading', () => {
    const body = [
      'Upgrade the parser and bound aliases.',
      '',
      'Validation: `npm run check`.',
      '',
      'Co-authored-by: Example <example@example.invalid>',
    ].join('\n');
    assert.equal(summaryFromBody(body), '');
  });

  it('groups immutable squash metadata in a fixed category order', () => {
    const notes = renderReleaseNotes({
      version: '10.5.0',
      previousTag: 'v10.4.0',
      commits: [
        commit(
          'a',
          'fix(runtime): preserve target changes (#41)',
          '## Summary\n\nKeep newer target changes during repair.\n\n## Testing\n\nPassed.'
        ),
        commit(
          'b',
          'feat(ui)!: add the workspace (#42)',
          '## Summary\n\nAdd profile editing and run replay.\n\n## Testing\n\nPassed.'
        ),
      ],
    });
    assert.equal(
      notes,
      [
        'Changes since [v10.4.0](https://github.com/the-open-engine/zeroshot/compare/v10.4.0...v10.5.0).',
        '',
        '## Breaking changes',
        '',
        '### Add the workspace ([#42](https://github.com/the-open-engine/zeroshot/pull/42))',
        '',
        'Add profile editing and run replay.',
        '',
        '## Fixes',
        '',
        '### Preserve target changes ([#41](https://github.com/the-open-engine/zeroshot/pull/41))',
        '',
        'Keep newer target changes during repair.',
        '',
        '## Upgrade',
        '',
        '```bash',
        'npm install -g @the-open-engine-company/zeroshot@10.5.0',
        '```',
        '',
      ].join('\n')
    );
  });
});

describe('release-note validation', () => {
  it('rejects release commits that cannot be traced to a pull request', () => {
    assert.throws(
      () => parseReleaseCommit(commit('c', 'feat: unreviewed change', 'Summary.')),
      /not a Conventional Commit squash title/
    );
  });

  it('rejects a pull request with no user-facing summary', () => {
    assert.throws(
      () =>
        parseReleaseCommit(
          commit('d', 'fix: hide internal validation (#43)', '## Summary\n\n## Testing\n\nPassed.')
        ),
      /has no release summary/
    );
  });

  it('recovers the one immutable plain-heading summary without weakening later commits', () => {
    const body = [
      'Summary',
      '- Route authority failures directly to refusal.',
      '',
      'Validation',
      '- cargo test --workspace',
    ].join('\n');
    const historical = {
      hash: '78b7baa2d88dcd6a7640d8cf06d6fbce94d91f42',
      subject: 'fix(delivery): route authority failures without repair (#1141)',
      body,
    };

    assert.equal(
      parseReleaseCommit(historical).summary,
      '- Route authority failures directly to refusal.'
    );
    assert.throws(
      () => parseReleaseCommit({ ...historical, hash: 'a'.repeat(40) }),
      /has no release summary/
    );
  });

  it('classifies both Conventional Commit breaking footer forms', () => {
    for (const footer of ['BREAKING CHANGE:', 'BREAKING-CHANGE:']) {
      const body = [
        '## Summary',
        '',
        'Replace the transport.',
        '',
        '## Testing',
        '',
        'Passed.',
        '',
        `${footer} Clients must reconnect.`,
        '',
        'Closes: #123',
        '',
        '---------',
        '',
        'Co-authored-by: Example <example@example.invalid>',
      ].join('\n');
      const parsed = parseReleaseCommit(commit('e', 'feat(api): replace transport (#44)', body));
      assert.equal(parsed.category, 'breaking');
    }
  });

  it('ignores breaking-looking prose and fenced examples', () => {
    for (const testing of [
      'Expected output:\nBREAKING CHANGE: fixture text',
      '```text\nBREAKING CHANGE: fixture text\n```',
    ]) {
      const parsed = parseReleaseCommit(
        commit(
          'f',
          'feat(api): document transport examples (#45)',
          `## Summary\n\nDocument transport examples.\n\n## Testing\n\n${testing}`
        )
      );
      assert.equal(parsed.category, 'feat');
    }
  });
});

describe('historical release metadata recovery', () => {
  it('recovers the immutable Dependabot update without accepting later missing summaries', () => {
    const historical = {
      hash: '0d688ae5773febd2e6c81def59790b04c9e8fd58',
      subject: 'chore(deps-dev): bump prettier from 3.9.6 to 3.9.8 (#1136)',
      body: 'Bumps the development-dependencies group with 1 update:\n[prettier].',
    };
    assert.equal(
      parseReleaseCommit(historical).summary,
      'Update the development formatter Prettier from 3.9.6 to 3.9.8.'
    );
    assert.throws(
      () => parseReleaseCommit({ ...historical, hash: 'b'.repeat(40) }),
      /has no release summary/
    );
    assert.equal(
      parseReleaseCommit({ ...historical, body: '## Summary\n\nExplicit summary.' }).summary,
      'Explicit summary.'
    );
  });
});

describe('release-note Git history', () => {
  it('uses the preceding canonical tag and immutable first-parent range', () => {
    const repository = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-release-notes-'));
    try {
      git(repository, 'init', '--quiet', '--initial-branch=main');
      git(repository, 'config', 'user.name', 'release-test');
      git(repository, 'config', 'user.email', 'release-test@example.invalid');
      git(
        repository,
        'commit',
        '--quiet',
        '--allow-empty',
        '-m',
        'chore: establish release baseline (#39)',
        '-m',
        '## Summary\n\nEstablish the release baseline.'
      );
      git(repository, 'tag', 'v10.3.0');
      git(repository, 'tag', 'v10.4.1');
      git(
        repository,
        'commit',
        '--quiet',
        '--allow-empty',
        '-m',
        'fix(cli): publish the preceding release (#40)',
        '-m',
        '## Summary\n\nPublish the preceding release.'
      );
      git(repository, 'tag', 'v10.4.0');
      git(
        repository,
        'commit',
        '--quiet',
        '--allow-empty',
        '-m',
        'feat(cli): add deterministic notes (#41)',
        '-m',
        '## Summary\n\nGenerate notes from Git history.\n\n## Testing\n\nPassed.'
      );
      const releaseCommit = git(repository, 'rev-parse', 'HEAD');
      git(repository, 'tag', 'v10.5.0');

      const first = generateReleaseNotes({ repository, version: '10.5.0', commit: releaseCommit });
      const second = generateReleaseNotes({ repository, version: '10.5.0', commit: releaseCommit });

      assert.deepEqual(second, first);
      assert.equal(first.commits, 1);
      assert.equal(first.previousTag, 'v10.4.0');
      assert.match(first.notes, /Generate notes from Git history\./);
      assert.doesNotMatch(first.notes, /Passed\./);
    } finally {
      fs.rmSync(repository, { recursive: true, force: true });
    }
  });
});
