const assert = require('assert');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { execFileSync } = require('child_process');

const {
  commandProofToQualityGate,
  mergeCommandProofs,
  normalizeCommandProofs,
  parseCommandProofsFromText,
  resolveConfiguredCommandProofs,
} = require('../../src/command-proofs');

describe('command proofs', function () {
  it('parses fenced zeroshot-command-proofs JSON from issue text', function () {
    const text = [
      '# Task',
      '',
      '```zeroshot-command-proofs',
      '[',
      '  {',
      '    "id": "opcore-ci",',
      '    "profile": "ci-equivalent",',
      '    "scope": "repo",',
      '    "description": "Opcore local CI",',
      '    "command": "bash ./scripts/ci/run-local-ci-equivalent.sh"',
      '  }',
      ']',
      '```',
    ].join('\n');

    assert.deepStrictEqual(parseCommandProofsFromText(text), [
      {
        id: 'opcore-ci',
        profile: 'ci-equivalent',
        scope: 'repo',
        description: 'Opcore local CI',
        command: 'bash ./scripts/ci/run-local-ci-equivalent.sh',
      },
    ]);
  });

  it('returns no proofs when the issue text has no proof block', function () {
    assert.deepStrictEqual(parseCommandProofsFromText('# Task\n\nNo proof config.'), []);
  });

  it('throws a clear error for malformed proof blocks', function () {
    assert.throws(
      () => parseCommandProofsFromText('```zeroshot-command-proofs\n{ nope\n```'),
      /Invalid zeroshot-command-proofs JSON/
    );
  });

  it('normalizes only proofs with id, profile, and command', function () {
    assert.deepStrictEqual(
      normalizeCommandProofs([
        {
          id: ' opcore-ci ',
          profile: ' ci-equivalent ',
          command: ' bash ./scripts/ci/run-local-ci-equivalent.sh ',
        },
        { id: 'missing-command', profile: 'ci-equivalent' },
        'not-an-object',
      ]),
      [
        {
          id: 'opcore-ci',
          profile: 'ci-equivalent',
          command: 'bash ./scripts/ci/run-local-ci-equivalent.sh',
        },
      ]
    );
  });

  it('merges proofs by id with later sources taking precedence', function () {
    assert.deepStrictEqual(
      mergeCommandProofs(
        [
          {
            id: 'opcore-ci',
            profile: 'ci-equivalent',
            command: 'npm test',
            description: 'repo default',
          },
        ],
        [
          {
            id: 'opcore-ci',
            profile: 'ci-equivalent',
            command: 'bash ./scripts/ci/run-local-ci-equivalent.sh',
            description: 'issue override',
          },
        ]
      ),
      [
        {
          id: 'opcore-ci',
          profile: 'ci-equivalent',
          command: 'bash ./scripts/ci/run-local-ci-equivalent.sh',
          description: 'issue override',
        },
      ]
    );
  });

  it('converts proofs into required quality gates', function () {
    assert.deepStrictEqual(
      commandProofToQualityGate({
        id: 'opcore-ci',
        profile: 'ci-equivalent',
        scope: 'repo',
        description: 'Opcore local CI',
        command: 'bash ./scripts/ci/run-local-ci-equivalent.sh',
      }),
      {
        id: 'opcore-ci',
        profile: 'ci-equivalent',
        scope: 'repo',
        description: 'Opcore local CI',
        command: 'bash ./scripts/ci/run-local-ci-equivalent.sh',
        commandProof: true,
      }
    );
  });

  it('resolves command proofs from repo settings', function () {
    const repoDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-command-proofs-'));
    try {
      execFileSync('git', ['init'], { cwd: repoDir, stdio: 'ignore' });
      fs.mkdirSync(path.join(repoDir, '.zeroshot'), { recursive: true });
      fs.writeFileSync(
        path.join(repoDir, '.zeroshot', 'settings.json'),
        JSON.stringify({
          ship: {
            commandProofs: [
              {
                id: 'opcore-ci',
                profile: 'ci-equivalent',
                command: 'bash ./scripts/ci/run-local-ci-equivalent.sh',
              },
            ],
          },
        })
      );

      assert.deepStrictEqual(resolveConfiguredCommandProofs({}, { cwd: repoDir }), [
        {
          id: 'opcore-ci',
          profile: 'ci-equivalent',
          command: 'bash ./scripts/ci/run-local-ci-equivalent.sh',
        },
      ]);
    } finally {
      fs.rmSync(repoDir, { recursive: true, force: true });
    }
  });
});
