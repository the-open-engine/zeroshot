'use strict';

// docs/llms.txt, the README and the first-run guide are what people and their agents copy, often
// over plain HTTP with no JavaScript. These checks keep them readable that way and true to the CLI.
// zeroshot/src/native_v2_cli/tests/documented_examples.rs also runs every documented `zeroshot run`
// through the real parser and the first-run runtimes through `--validate-only` admission.

const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { it } = require('node:test');
const jsYaml = require('js-yaml');
const { runWorkflowStep } = require('./workflow-http-fixture');
const { renderReleaseNotes } = require('../../scripts/release-notes');

const root = path.resolve(__dirname, '../..');
const read = (relative) => fs.readFileSync(path.join(root, relative), 'utf8');
const LLMS = 'docs/llms.txt';
const FIRST_RUN = 'docs/getting-started/first-run.md';
// The generated CLI reference shows usage text, not examples.
const GENERATED = new Set(['docs/zeroshot-cli.md']);

function documentationFiles(directory = 'docs') {
  return fs.readdirSync(path.join(root, directory), { withFileTypes: true }).flatMap((entry) => {
    const relative = `${directory}/${entry.name}`;
    if (entry.isDirectory()) return documentationFiles(relative);
    return /\.(md|txt)$/.test(entry.name) ? [relative] : [];
  });
}

function shellWords(command) {
  const words = [];
  let current = '';
  let quote = null;
  let started = false;
  for (const character of command) {
    if (quote) {
      if (character === quote) quote = null;
      else current += character;
    } else if (character === "'" || character === '"') {
      quote = character;
      started = true;
    } else if (character === '#' && !started) {
      break;
    } else if (/\s/.test(character)) {
      if (started) words.push(current);
      current = '';
      started = false;
    } else {
      current += character;
      started = true;
    }
  }
  if (started) words.push(current);
  return words;
}

// Every `zeroshot run` inside a fenced block, with backslash continuations joined.
function documentedRuns(file) {
  const lines = read(file).split('\n');
  const runs = [];
  let fenced = false;
  for (let index = 0; index < lines.length; index += 1) {
    const trimmed = lines[index].trim();
    if (trimmed.startsWith('```')) {
      fenced = !fenced;
      continue;
    }
    const command = trimmed.replace(/^\$ /, '');
    if (!fenced || !command.startsWith('zeroshot run ')) continue;
    const location = `${file}:${index + 1}`;
    let joined = command;
    while (joined.endsWith('\\') && index + 1 < lines.length) {
      index += 1;
      joined = `${joined.slice(0, -1)} ${lines[index].trim()}`;
    }
    runs.push({ location, words: shellWords(joined) });
  }
  return runs;
}

const exampleFiles = () =>
  ['README.md', ...documentationFiles()].filter((file) => !GENERATED.has(file));
const allRuns = () => exampleFiles().flatMap(documentedRuns);
const flagName = (word) => word.split('=')[0];

function localReadmeTargets(markdown) {
  const inline = [...markdown.matchAll(/\]\(\s*(?:<([^>]+)>|([^\s)]+))/g)].map(
    (match) => match[1] ?? match[2]
  );
  const references = [...markdown.matchAll(/^ {0,3}\[[^\]\n]+\]:\s*(?:<([^>]+)>|(\S+))/gm)].map(
    (match) => match[1] ?? match[2]
  );
  const html = [...markdown.matchAll(/<[a-z][^>]*>/gi)].flatMap(([tag]) =>
    [...tag.matchAll(/\b(href|src|srcset)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))/gi)].flatMap(
      ([, attribute, doubleQuoted, singleQuoted, unquoted]) => {
        const value = doubleQuoted ?? singleQuoted ?? unquoted;
        return attribute.toLowerCase() === 'srcset'
          ? value.split(',').map((candidate) => candidate.trim().split(/\s+/)[0])
          : [value];
      }
    )
  );
  return [...inline, ...references, ...html].filter(
    (target) => target && !/^(?:[a-z][a-z\d+.-]*:|\/\/|#)/i.test(target)
  );
}

it('finds local inline, reference-style, and HTML link/image targets', () => {
  const markdown = [
    '[inline](docs/inline.md#intro)',
    '![image](docs/image.png)',
    '[guide][guide]',
    '[guide]: docs/reference.md "Guide"',
    '[space]: <docs/with space.md>',
    '<a href="docs/double.md">double</a>',
    "<a href='docs/single.md'>single</a>",
    '<a href=docs/unquoted.md>unquoted</a>',
    '<img src="docs/photo.png" srcset="docs/small.png 1x, docs/large.png 2x">',
    '[web](https://example.com)',
    '[section](#intro)',
    '<a href="mailto:dev@example.com">email</a>',
  ].join('\n');
  assert.deepEqual(localReadmeTargets(markdown), [
    'docs/inline.md#intro',
    'docs/image.png',
    'docs/reference.md',
    'docs/with space.md',
    'docs/double.md',
    'docs/single.md',
    'docs/unquoted.md',
    'docs/photo.png',
    'docs/small.png',
    'docs/large.png',
  ]);
});

it('README links and images point to existing local files', () => {
  const local = localReadmeTargets(read('README.md'));
  assert.ok(local.length > 0, 'README must link its local documentation');
  for (const target of local) {
    const filename = decodeURIComponent(target.split(/[?#]/)[0]);
    assert.ok(
      fs.existsSync(path.resolve(root, filename)),
      `README references missing file: ${target}`
    );
  }
});

it('README links to the canonical self-hosting guide', () => {
  assert.ok(localReadmeTargets(read('README.md')).includes('docker/zeroshot-target/README.md'));
});

// JSON examples in fenced blocks titled `name` or `name for ...`.
function titledJson(file, name) {
  const pattern = /```json title="([^"]+)"\n([\s\S]*?)\n\s*```/g;
  return [...read(file).matchAll(pattern)]
    .filter(([, title]) => title === name || title.startsWith(`${name} for `))
    .map(([, title, body]) => ({ title, value: JSON.parse(body) }));
}

// Package registries, HTTP-only agents and the CLI need an available product description.
// Preserve the distinction between the customizable engine and one built-in graph without
// freezing marketing copy to a particular slogan or phrase.
const DEFAULT_GRAPH_LEAD = /^.{0,80}\bone agent implements\b/i;

// The first prose paragraph after the title, joined onto one line.
function firstParagraph(markdown) {
  return markdown
    .split(/\n\s*\n/)
    .find((block) => block.trim() && !block.startsWith('#'))
    ?.replace(/\s+/g, ' ');
}

function productDescriptions() {
  const lede = read('docs/index.md').match(/<p class="docs-lede">\n([\s\S]*?)\n<\/p>/);
  return [
    [`${LLMS} summary`, read(LLMS).match(/^> (.*)$/m)?.[1]],
    ['npm description', JSON.parse(read('npm/zeroshot/package.json')).description],
    ['PyPI description', read('sdks/python/pyproject.toml').match(/^description = "(.*)"$/m)?.[1]],
    ['docs site description', read('mkdocs.yml').match(/^site_description: (.*)$/m)?.[1]],
    ['docs home lede', lede?.[1].replace(/\s+/g, ' ')],
    ['npm README', firstParagraph(read('npm/zeroshot/README.md'))],
    ['PyPI README', read('sdks/python/README.md').match(/^zeroshot is[\s\S]*?\.\s/m)?.[0]],
    [
      'zeroshot --help',
      read('docs/zeroshot-cli.md').match(/## `zeroshot`\n\n```text\n(.*)\n/)?.[1],
    ],
  ];
}

it('product descriptions are available and do not reduce the engine to one fixed graph', () => {
  for (const [surface, text] of productDescriptions()) {
    assert.ok(text, `${surface} was not found`);
    assert.doesNotMatch(
      text,
      DEFAULT_GRAPH_LEAD,
      `${surface} describes the built-in review loop as the product`
    );
  }
});

it('llms.txt links only to pages an agent can read without JavaScript', () => {
  const links = [...read(LLMS).matchAll(/https:\/\/zeroshot\.sh\/[^\s)>\]]*/g)].map(
    ([url]) => url.split('#')[0]
  );
  assert.ok(links.length > 0, `${LLMS} must link the docs`);
  for (const url of links) {
    if (url === 'https://zeroshot.sh/docs/llms.txt') continue;
    const page = url.match(/^https:\/\/zeroshot\.sh\/docs\/current\/(.*)$/);
    assert.ok(
      page,
      `${LLMS} links ${url}. Link /docs/current/: mike's version aliases such as /docs/stable/ ` +
        'are JavaScript redirects that HTTP-only agents cannot follow'
    );
    const slug = page[1].replace(/\/$/, '');
    const sources = slug ? [`docs/${slug}.md`, `docs/${slug}/index.md`] : ['docs/index.md'];
    assert.ok(
      sources.some((source) => fs.existsSync(path.join(root, source))),
      `${LLMS} links ${url}, but the docs have no ${sources.join(' or ')}`
    );
  }
});

it('llms.txt and the README state the delivery default the CLI applies', () => {
  const cliDefault = read('docs/zeroshot-cli.md').match(/Defaults to (\w+) for software-change/);
  assert.ok(
    cliDefault,
    'the generated CLI reference must state the software-change delivery default'
  );
  // One sentence per file states the default. A changed CLI default fails here until both change.
  const statements = {
    pull_request: {
      [LLMS]: '`software-change` opens a pull request by default',
      'README.md': '`software-change` defaults to PR delivery',
    },
  };
  const expected = statements[cliDefault[1]];
  assert.ok(expected, `the CLI default is now ${cliDefault[1]}; update both statements`);
  for (const [file, sentence] of Object.entries(expected)) {
    assert.ok(read(file).includes(sentence), `${file} must say: ${sentence}`);
  }
  assert.doesNotMatch(
    read(LLMS),
    /\(default\)/,
    `${LLMS} must state defaults in the checked sentence, not with "(default)" labels`
  );
});

it('no page contradicts the delivery default the CLI applies', () => {
  const cliDefault = read('docs/zeroshot-cli.md').match(/Defaults to (\w+) for software-change/)[1];
  assert.equal(cliDefault, 'pull_request', 'update this rule with the new software-change default');
  // A sentence about delivery that calls keeping the change local the default contradicts the CLI.
  const contradictions = exampleFiles().flatMap((file) =>
    read(file)
      .replace(/\s+/g, ' ')
      .split(/(?<=\.) /)
      .map((sentence, index, sentences) => [sentence, `${sentences[index - 1] ?? ''} ${sentence}`])
      .filter(
        ([sentence, context]) =>
          /\b(deliver|delivery|software-change)\b/i.test(context) &&
          /\bdefault\b[^.]*\blocal\b|\blocal\b[^.]*\(default\)/i.test(sentence)
      )
      .map(([sentence]) => `${file}: ${sentence.trim()}`)
  );
  assert.deepEqual(contradictions, [], 'software-change opens a pull request by default');
});

it('software-change examples name their delivery', () => {
  const implicit = allRuns()
    .filter(({ words }) =>
      words.some(
        (word, index) =>
          word === '--template=software-change' ||
          (word === '--template' && words[index + 1] === 'software-change')
      )
    )
    .filter(({ words }) =>
      words.every((word) => !['--delivery', '--push', '--pr', '--ship'].includes(flagName(word)))
    )
    .map(({ location }) => location);
  assert.deepEqual(
    implicit,
    [],
    'the default opens a pull request, which needs a committed Git checkout with a remote; ' +
      'add --delivery none, --push, --pr or --ship'
  );
});

it('documented run flags exist in the generated CLI reference', () => {
  const reference = read('docs/zeroshot-cli.md');
  const section = reference.slice(
    reference.indexOf('### `zeroshot run`'),
    reference.indexOf('### `zeroshot update`')
  );
  const known = new Set(
    [...section.matchAll(/^\s+(?:(-[a-zA-Z]), )?(--[a-z0-9-]+)/gm)].flatMap(([, short, long]) =>
      short ? [short, long] : [long]
    )
  );
  assert.ok(known.has('--validate-only'), 'the run section of the CLI reference was not found');
  const unknown = allRuns().flatMap(({ location, words }) =>
    words
      .filter((word) => word.startsWith('-') && !known.has(flagName(word)))
      .map((word) => `${location}: ${word}`)
  );
  assert.deepEqual(unknown, [], 'documented flags the CLI does not accept');
});

it('the llms.txt quick start matches the first-run guide', () => {
  const llms = read(LLMS);
  const echoed = (file) => {
    const match = llms.match(new RegExp(`echo '([^']+)' > ${file.replace('.', '\\.')}`));
    assert.ok(match, `${LLMS} must write ${file} in its quick start`);
    return JSON.parse(match[1]);
  };
  const guideJson = (name) => {
    const example = titledJson(FIRST_RUN, name).find(({ title }) => title === name);
    assert.ok(example, `${FIRST_RUN} must show the first-run ${name}`);
    return example.value;
  };
  assert.deepEqual(echoed('input.json'), guideJson('input.json'));
  assert.deepEqual(echoed('runtime.json'), guideJson('runtime.json'));

  const command = (file) => {
    const run = documentedRuns(file).find(({ words }) => words.includes('software-change'));
    assert.ok(run, `${file} must show the first run`);
    return run.words.filter((word) => word !== '--validate-only');
  };
  assert.deepEqual(command(LLMS), command(FIRST_RUN));
});

it('the first-run guide covers core harnesses from the canonical catalog', () => {
  const guide = 'docs/project/harness-adapters.md';
  const lanes = new Map(
    [...read(guide).matchAll(/^\|\s*`([^`]+)`\s*\|(.+?)\|\s*(Core|Contributed)\s*\|$/gm)].map(
      ([, harness, providers, distribution]) => [
        harness,
        {
          providers: [...providers.matchAll(/`([^`]+)`/g)].map(([, provider]) => provider),
          distribution,
        },
      ]
    )
  );
  assert.ok(lanes.has('codex'), `${guide} must contain the harness catalog`);
  const runtimes = titledJson(FIRST_RUN, 'runtime.json');
  for (const { title, value } of runtimes) {
    assert.ok(
      lanes.get(value.harness)?.providers.includes(value.provider),
      `${FIRST_RUN} "${title}" uses ${value.harness}/${value.provider}, which ${guide} rejects`
    );
  }
  const examples = new Set(runtimes.map(({ value }) => value.harness));
  for (const [harness, { distribution }] of lanes) {
    if (distribution === 'Core') {
      assert.ok(examples.has(harness), `${FIRST_RUN} needs a runtime.json example for ${harness}`);
    }
  }
  assert.ok(
    read(FIRST_RUN).includes('../project/harness-adapters.md'),
    `${FIRST_RUN} must link to the complete harness catalog`
  );
});

// The updater in 10.4.0 through 10.8.0 accepts only `zeroshot` in a release archive, and every
// archive since v10.9.0 also contains `restic`. No release can change an installed updater, so the
// places that explain updating must tell those users to reinstall once through npm.
const FIRST_STRANDED = /\b10\.4\b/;
const LAST_STRANDED = /\b10\.8\b/;
const STRANDED_FAILURE = /unexpected entry `?restic\b/;
const NPM_INSTALL = 'npm (?:install|i) (?:-g|--global) @the-open-engine-company/zeroshot@';

// Markdown sections split at every heading outside fenced blocks.
function sections(markdown) {
  const result = [[]];
  let fenced = false;
  for (const line of markdown.split('\n')) {
    if (/^\s*(```|~~~)/.test(line)) fenced = !fenced;
    if (!fenced && /^#{1,6}\s/.test(line)) result.push([]);
    result.at(-1).push(line);
  }
  return result.map((lines) => lines.join('\n'));
}

it('the install guide tells 10.4.0-10.8.0 users to reinstall through npm once', () => {
  const guide = 'docs/getting-started/install.md';
  const section = sections(read(guide)).find(
    (text) => FIRST_STRANDED.test(text) && LAST_STRANDED.test(text)
  );
  assert.ok(section, `${guide} does not say that 10.4.0-10.8.0 cannot update themselves`);
  assert.match(section, STRANDED_FAILURE);
  assert.match(section, new RegExp(`${NPM_INSTALL}latest\\b`));
});

it('release notes tell 10.4.0-10.8.0 users how to upgrade', () => {
  const notes = renderReleaseNotes({
    version: '11.2.0',
    previousTag: 'v11.1.0',
    commits: [
      {
        hash: 'a'.repeat(40),
        subject: 'fix(cli): keep release archives verified (#50)',
        body: '## Summary\n\nKeep release archives verified.\n\n## Testing\n\nPassed.',
      },
    ],
  });
  const start = notes.indexOf('## Upgrade\n');
  assert.notEqual(start, -1, 'release notes have no Upgrade section');
  const upgrade = notes.slice(start);
  assert.match(upgrade, FIRST_STRANDED);
  assert.match(upgrade, LAST_STRANDED);
  assert.match(upgrade, STRANDED_FAILURE);
  assert.match(upgrade, new RegExp(`${NPM_INSTALL}(?:latest|11\\.2\\.0)\\b`));
});

it('extracts commands the way a shell reads them', () => {
  assert.deepEqual(shellWords(`zeroshot run --title "Add JSON" --input 'in put.json' # note`), [
    'zeroshot',
    'run',
    '--title',
    'Add JSON',
    '--input',
    'in put.json',
  ]);
});

// The strict Docs lane runs this file even when only the publication workflow changes.
const docsPublicationCommit = 'a'.repeat(40);
const docsPublicationManifest = {
  docsVersion: 'v15.0',
  productVersion: '15.0.0',
  sourceCommit: docsPublicationCommit,
};
const docsPublicationOptions = { skip: process.platform === 'win32' };
const docsPublicationStep = 'Verify live documentation manifest';

function runDocsPublicationVerification(t, replies, current = false) {
  return runWorkflowStep(t, {
    workflow: '.github/workflows/docs.yml',
    job: 'publish',
    step: docsPublicationStep,
    prefix: 'export http_proxy="$HTTP_PROXY"\nsleep() { echo "sleep:$1"; }\n',
    environment: {
      DOCS_SITE_URL: 'http://pages.localhost/zeroshot/',
      DOCS_VERSION: current ? 'current' : 'v15.0',
      ZEROSHOT_PRODUCT_DOCS_VERSION: current ? '' : '15.0.0',
      SOURCE_COMMIT: docsPublicationCommit,
    },
    respond(_request, response, index) {
      const reply = replies[Math.min(index, replies.length - 1)];
      response.writeHead(reply.status ?? 200);
      response.end(typeof reply.body === 'string' ? reply.body : JSON.stringify(reply.body));
    },
  });
}

function assertDocsPublicationExhausted(result) {
  assert.notEqual(result.code, 0);
  assert.equal(result.requests.length, 120);
  assert.equal((result.stdout.match(/sleep:5/g) ?? []).length, 119);
  assert.match(result.stdout, /did not match after 120 attempts/);
  assert.doesNotMatch(result.stdout, /Verified public documentation/);
}

it(
  'checks the exact released public manifest after the official deployment',
  docsPublicationOptions,
  async (t) => {
    const workflow = jsYaml.load(read('.github/workflows/docs.yml'));
    const steps = workflow.jobs.publish.steps;
    const verify = steps.find((step) => step.name === docsPublicationStep);
    assert.ok(steps.indexOf(verify) > steps.findIndex((step) => step.id === 'deployment'));
    assert.equal(verify.env.DOCS_SITE_URL, '${{ steps.deployment.outputs.page_url }}');
    assert.notEqual(verify['continue-on-error'], true);
    const result = await runDocsPublicationVerification(t, [
      { body: { ...docsPublicationManifest, publisherCommit: 'b'.repeat(40) } },
    ]);
    assert.equal(result.code, 0, result.stderr);
    assert.deepEqual(result.requests, [
      {
        method: 'GET',
        url: 'http://pages.localhost/zeroshot/v15.0/manifest.json',
        body: '',
      },
    ]);
    assert.match(result.stdout, /Verified public documentation manifest/);
    assert.doesNotMatch(result.stdout, /sleep:/);
  }
);

it('requires an explicit null product version for Current', docsPublicationOptions, async (t) => {
  const current = { ...docsPublicationManifest, docsVersion: 'current', productVersion: null };
  const success = await runDocsPublicationVerification(t, [{ body: current }], true);
  assert.equal(success.code, 0, success.stderr);
  assert.equal(success.requests[0].url, 'http://pages.localhost/zeroshot/current/manifest.json');
  delete current.productVersion;
  assertDocsPublicationExhausted(
    await runDocsPublicationVerification(t, [{ body: current }], true)
  );
});

it(
  'retries unavailable, stale and malformed responses until the exact manifest is live',
  docsPublicationOptions,
  async (t) => {
    const result = await runDocsPublicationVerification(t, [
      { status: 404, body: {} },
      { status: 503, body: {} },
      { body: { ...docsPublicationManifest, sourceCommit: 'b'.repeat(40) } },
      { body: '<html>deployment pending</html>' },
      { body: docsPublicationManifest },
    ]);
    assert.equal(result.code, 0, result.stderr);
    assert.equal(result.requests.length, 5);
    assert.ok(
      result.requests.every(
        ({ method, url }) =>
          method === 'GET' && url === 'http://pages.localhost/zeroshot/v15.0/manifest.json'
      )
    );
    assert.equal((result.stdout.match(/sleep:5/g) ?? []).length, 4);
  }
);

it(
  'fails bounded polling for wrong identities and malformed HTTP 200 manifests',
  docsPublicationOptions,
  async (t) => {
    for (const body of [
      { ...docsPublicationManifest, docsVersion: 'v14.0' },
      { ...docsPublicationManifest, productVersion: '14.0.0' },
      { ...docsPublicationManifest, sourceCommit: 'b'.repeat(40) },
      [],
      'not json',
    ]) {
      assertDocsPublicationExhausted(await runDocsPublicationVerification(t, [{ body }]));
    }
  }
);

it(
  'fails bounded polling when public documentation stays unavailable',
  docsPublicationOptions,
  async (t) => {
    for (const status of [404, 503]) {
      assertDocsPublicationExhausted(
        await runDocsPublicationVerification(t, [{ status, body: {} }])
      );
    }
  }
);
