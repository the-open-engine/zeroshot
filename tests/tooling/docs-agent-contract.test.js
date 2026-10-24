'use strict';

// docs/llms.txt, the README and the first-run guide are what people and their agents copy, often
// over plain HTTP with no JavaScript. These checks keep them readable that way and true to the CLI.
// zeroshot/src/native_v2_cli/tests/documented_examples.rs also runs every documented `zeroshot run`
// through the real parser and the first-run runtimes through `--validate-only` admission.

const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { it } = require('node:test');

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

// JSON examples in fenced blocks titled `name` or `name for ...`.
function titledJson(file, name) {
  const pattern = /```json title="([^"]+)"\n([\s\S]*?)\n\s*```/g;
  return [...read(file).matchAll(pattern)]
    .filter(([, title]) => title === name || title.startsWith(`${name} for `))
    .map(([, title, body]) => ({ title, value: JSON.parse(body) }));
}

// Agents and package registries show these instead of the product page, so each one leads with
// what makes Zeroshot worth adopting: the team's own QA process, defined as a graph and enforced.
// The shared standard is "Every surface that describes the product" in the company repo's
// message skill (.claude/skills/message/references/product-messaging.md).
const PITCH = "your team's QA process";
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
    ['PyPI README', read('sdks/python/README.md').match(/^Zeroshot enforces[\s\S]*?\.\s/m)?.[0]],
    [
      'zeroshot --help',
      read('docs/zeroshot-cli.md').match(/## `zeroshot`\n\n```text\n(.*)\n/)?.[1],
    ],
  ];
}

it('every product description sells the team’s own enforced QA process', () => {
  for (const [surface, text] of productDescriptions()) {
    assert.ok(text, `${surface} was not found`);
    assert.ok(
      text.includes(PITCH) && /\bgraph\b/.test(text),
      `${surface} must lead with ${PITCH}, defined as a graph; it reads: ${text}`
    );
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

it('the llms.txt quick start matches the README first run', () => {
  const llms = read(LLMS);
  const echoed = (file) => {
    const match = llms.match(new RegExp(`echo '([^']+)' > ${file.replace('.', '\\.')}`));
    assert.ok(match, `${LLMS} must write ${file} in its quick start`);
    return JSON.parse(match[1]);
  };
  const readme = read('README.md');
  const readmeJson = (key) => {
    const block = [...readme.matchAll(/```json\n([\s\S]*?)\n```/g)].find(([, body]) =>
      body.includes(`"${key}"`)
    );
    assert.ok(block, `README.md must show the first-run ${key} JSON`);
    return JSON.parse(block[1]);
  };
  assert.deepEqual(echoed('input.json'), readmeJson('task'));
  assert.deepEqual(echoed('runtime.json'), readmeJson('harness'));

  const command = (file) => {
    const run = documentedRuns(file).find(({ words }) => words.includes('software-change'));
    assert.ok(run, `${file} must show the first run`);
    return run.words.filter((word) => word !== '--validate-only');
  };
  assert.deepEqual(command(LLMS), command('README.md'));
});

it('the first-run guide shows a runtime for every harness', () => {
  const table = read('docs/reference/runtime-plan.md')
    .split('\n## Harness and provider\n')[1]
    .split('\n## ')[0];
  const lanes = new Map(
    [...table.matchAll(/^\| `(\w+)`\s+\| (.+?) \|$/gm)]
      .filter(([, harness]) => harness !== 'harness')
      .map(([, harness, providers]) => [
        harness,
        [...providers.matchAll(/`(\w+)`/g)].map(([, provider]) => provider),
      ])
  );
  assert.ok(lanes.has('codex'), 'the RuntimePlan harness table was not found');
  const runtimes = titledJson(FIRST_RUN, 'runtime.json');
  for (const { title, value } of runtimes) {
    assert.ok(
      lanes.get(value.harness)?.includes(value.provider),
      `${FIRST_RUN} "${title}" uses ${value.harness}/${value.provider}, which the RuntimePlan table rejects`
    );
  }
  assert.deepEqual(
    [...new Set(runtimes.map(({ value }) => value.harness))].sort(),
    [...lanes.keys()].sort(),
    `${FIRST_RUN} needs one runtime.json example per harness`
  );
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
