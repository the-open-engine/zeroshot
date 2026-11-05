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

const recoveredReadmeSummaries = [
  {
    hash: 'ed424368780b4c052fa8ad2fe8b1462036c3aa9b',
    subject: 'docs(readme): lead with using zeroshot through your agent (#1334)',
    summary:
      'Rewrite the README around installing Zeroshot and using its graph-design, planning and run skills through a coding agent.',
  },
  {
    hash: '968ec77faa73b70dc69cd1f4837224a17bb9131c',
    subject:
      'docs(readme): add the blog link, the starred-by strip, the problem and where work goes (#1335)',
    summary:
      'Add the README blog link and starred-by banner, and explain local, branch, PR and merge delivery.',
  },
  {
    hash: 'd99ac445cd8fc14441e2d90adc59edb3bb02efbc',
    subject: 'docs(readme): read as a dev tool, not a team pitch (#1336)',
    summary:
      'Address individual developers in the README introduction and keep shared Platform workflows in a separate team section.',
  },
  {
    hash: '6a8cf31f0e404131683a0ead9ac5a992bdb5de5a',
    subject: 'docs(readme): open with why reviews must be independent and enforced (#1337)',
    summary:
      'Explain independent reviews in the README, including fresh reviewer sessions and enforced repair cycles.',
  },
  {
    hash: '40a6d49b6521bbb6f1ef36eb9bcc18f9928106b3',
    subject: 'docs(readme): say the graph is customizable down to the model per step (#1338)',
    summary:
      "Clarify in the README that graphs customize workflow steps and select each step's tool and model.",
  },
  {
    hash: '305ab185ab0a5193e494f0601103b24816ab21a1',
    subject: 'docs(readme): open with why single-agent sessions fail (#1339)',
    summary:
      "Explain why independent reviewers check single-agent work and return failed checks through the graph's repair loop.",
  },
  {
    hash: '516447fc32004bce940439dbc355030aca22d22a',
    subject:
      'docs(readme): add the skills, an architecture diagram and a command reference (#1340)',
    summary:
      'Document the four installed skills, add an architecture diagram, and list common CLI commands in the README.',
  },
  {
    hash: '8523568a8b4cca268e4302500ab8cacb088f62ae',
    subject: 'docs(readme): use a compact architecture diagram (#1341)',
    summary:
      "Replace the README's boxed architecture diagram with a compact flow through admission, execution, adapters, durable state and delivery.",
  },
];

describe('historical release metadata recovery', () => {
  it('recovers only the recorded README summaries', () => {
    for (const historical of [
      {
        hash: 'e6faeb5a9c41061da7e820e2e6bc394a0c8ca606',
        subject: 'docs(readme): write the name as zeroshot and open with the QA process (#1232)',
        summary:
          'Write the README name as zeroshot and introduce its customizable QA graph with existing coding agents.',
      },
      {
        hash: '6da880abae8fd454c60f80d2b23d94939c194859',
        subject: "docs: say Zeroshot runs the user's existing coding agent (#1191)",
        summary:
          "Clarify that Zeroshot runs the user's existing coding agent as workers and reviewers.",
      },
      {
        hash: '101ec9d9f50d6f7a5d871e5bc655dc03bfc54a99',
        subject: 'docs: label the custom topology as an example (#1192)',
        summary:
          "Clarify that the README's custom bugfinder/E2E topology is an example and only the review loop is built in.",
      },
      ...recoveredReadmeSummaries,
    ]) {
      const input = { ...historical, body: 'README change without a Summary heading.' };
      assert.equal(parseReleaseCommit(input).summary, historical.summary);
      assert.throws(
        () => parseReleaseCommit({ ...input, hash: 'b'.repeat(40) }),
        /has no release summary/
      );
      assert.equal(
        parseReleaseCommit({ ...input, body: '## Summary\n\nExplicit summary.' }).summary,
        'Explicit summary.'
      );
    }
  });

  it('recovers recorded Dependabot updates without accepting later missing summaries', () => {
    for (const update of [
      {
        hash: '0d688ae5773febd2e6c81def59790b04c9e8fd58',
        subject: 'chore(deps-dev): bump prettier from 3.9.6 to 3.9.8 (#1136)',
        summary: 'Update the development formatter Prettier from 3.9.6 to 3.9.8.',
      },
      {
        hash: '7b098aab235f9cb1ee047470f886a73ff4d95bf3',
        subject:
          'chore(deps-dev): bump prettier from 3.9.9 to 3.9.10 in the development-dependencies group (#1266)',
        summary: 'Update the development formatter Prettier from 3.9.9 to 3.9.10.',
      },
    ]) {
      const historical = {
        ...update,
        body: 'Bumps the development-dependencies group with 1 update:\n[prettier].',
      };
      assert.equal(parseReleaseCommit(historical).summary, update.summary);
      assert.throws(
        () => parseReleaseCommit({ ...historical, hash: 'b'.repeat(40) }),
        /has no release summary/
      );
      assert.equal(
        parseReleaseCommit({ ...historical, body: '## Summary\n\nExplicit summary.' }).summary,
        'Explicit summary.'
      );
    }
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
