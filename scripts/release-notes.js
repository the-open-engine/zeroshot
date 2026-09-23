#!/usr/bin/env node
'use strict';

const childProcess = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');

const REPOSITORY_URL = 'https://github.com/the-open-engine/zeroshot';
const CANONICAL_TAG = /^v(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)$/;
const RELEASE_VERSION = /^(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)$/;
const RELEASE_COMMIT = /^[0-9a-f]{40}$/;
const RELEASE_SUBJECT =
  /^(?<type>[a-z]+)(?:\((?<scope>[^()\r\n]+)\))?(?<breaking>!)?: (?<title>.+) \(#(?<pull>[1-9][0-9]*)\)$/;
const PLAIN_SUMMARY_COMMIT_EXCEPTIONS = new Set(['78b7baa2d88dcd6a7640d8cf06d6fbce94d91f42']);
const CATEGORIES = Object.freeze([
  ['breaking', 'Breaking changes'],
  ['feat', 'Features'],
  ['fix', 'Fixes'],
  ['perf', 'Performance'],
  ['docs', 'Documentation'],
  ['maintenance', 'Maintenance'],
]);
const CATEGORY_KEYS = new Set(CATEGORIES.map(([key]) => key));

function parseArguments(argv) {
  const values = new Map();
  for (let index = 0; index < argv.length; index += 2) {
    const flag = argv[index];
    const value = argv[index + 1];
    if (!['--repo', '--version', '--commit', '--output'].includes(flag) || !value) {
      throw new Error(
        'usage: release-notes.js --repo <path> --version <X.Y.Z> ' +
          '--commit <sha> --output <path>'
      );
    }
    if (values.has(flag)) throw new Error(`duplicate argument: ${flag}`);
    values.set(flag, value);
  }
  for (const flag of ['--repo', '--version', '--commit', '--output']) {
    if (!values.has(flag)) throw new Error(`missing ${flag}`);
  }
  return {
    repository: path.resolve(values.get('--repo')),
    version: values.get('--version'),
    commit: values.get('--commit'),
    output: path.resolve(values.get('--output')),
  };
}

function runGit(repository, arguments_) {
  const result = childProcess.spawnSync('git', arguments_, {
    cwd: repository,
    encoding: 'utf8',
    maxBuffer: 16 * 1024 * 1024,
  });
  if (result.error) throw result.error;
  if (result.signal || result.status !== 0) {
    throw new Error(
      `git ${arguments_.join(' ')} failed: status=${result.status} ` +
        `signal=${result.signal || 'none'} stderr=${result.stderr.trim()}`
    );
  }
  return result.stdout.trimEnd();
}

function previousCanonicalTag(repository, releaseCommit, releaseTag) {
  const firstParentCommits = runGit(repository, [
    'rev-list',
    '--first-parent',
    releaseCommit,
  ]).split('\n');
  const tagsByCommit = new Map();
  const tags = runGit(repository, ['tag', '--list', 'v[0-9]*', '--merged', releaseCommit])
    .split('\n')
    .filter((tag) => CANONICAL_TAG.test(tag) && tag !== releaseTag);
  for (const tag of tags) {
    const tagCommit = runGit(repository, ['rev-list', '-n', '1', tag]);
    const commitTags = tagsByCommit.get(tagCommit) || [];
    commitTags.push(tag);
    tagsByCommit.set(tagCommit, commitTags);
  }
  for (const commit of firstParentCommits.slice(1)) {
    const commitTags = tagsByCommit.get(commit) || [];
    if (commitTags.length === 1) return commitTags[0];
    if (commitTags.length > 1) {
      throw new Error(
        `multiple canonical tags mark preceding commit ${commit}: ${commitTags.join(', ')}`
      );
    }
  }
  throw new Error(`no preceding canonical tag is reachable from ${releaseCommit}`);
}

function releaseCommits(repository, previousTag, releaseCommit) {
  const revisionList = runGit(repository, [
    'rev-list',
    '--first-parent',
    '--reverse',
    `${previousTag}..${releaseCommit}`,
  ]);
  const hashes = revisionList ? revisionList.split('\n') : [];
  if (hashes.length === 0) {
    throw new Error(`release range ${previousTag}..${releaseCommit} contains no commits`);
  }
  return hashes.map((hash) => {
    const fields = runGit(repository, ['show', '-s', '--format=%H%x00%s%x00%b', hash]).split('\0');
    if (fields.length !== 3 || fields[0] !== hash) {
      throw new Error(`cannot read immutable release metadata for ${hash}`);
    }
    return { hash: fields[0], subject: fields[1], body: fields[2] };
  });
}

function sectionFromBody(body, headingPattern, nextHeadingPattern) {
  const lines = body.replace(/\r\n/g, '\n').split('\n');
  const summaryHeading = lines.findIndex((line) => headingPattern.test(line));
  if (summaryHeading === -1) return '';
  const start = summaryHeading + 1;
  let end = lines.length;
  for (let index = start; index < lines.length; index += 1) {
    if (nextHeadingPattern.test(lines[index])) {
      end = index;
      break;
    }
  }
  return lines.slice(start, end).join('\n').trim();
}

function summaryFromBody(body) {
  return sectionFromBody(body, /^## Summary\s*$/i, /^##\s+/);
}

function plainSummaryFromBody(body) {
  return sectionFromBody(body, /^Summary\s*$/i, /^(?:Validation\s*$|##\s+)/i);
}

function hasBreakingFooter(body) {
  const footerToken = /^(?:BREAKING(?: CHANGE|-CHANGE)|[A-Za-z][A-Za-z0-9-]*)(?::[ \t]+| #[0-9])/;
  const blocks = body
    .replace(/\r\n/g, '\n')
    .trimEnd()
    .split(/\n[ \t]*\n/);
  let foundFooter = false;

  for (let index = blocks.length - 1; index >= 0; index -= 1) {
    const lines = blocks[index].split('\n');
    if (foundFooter && lines.every((line) => /^-{3,}$/.test(line.trim()))) continue;
    if (!footerToken.test(lines[0])) break;
    foundFooter = true;
    if (lines.some((line) => /^BREAKING(?: CHANGE|-CHANGE):[ \t]+/.test(line))) return true;
  }
  return false;
}

function parseReleaseCommit(commit) {
  const match = RELEASE_SUBJECT.exec(commit.subject);
  if (!match) {
    throw new Error(
      `${commit.hash} is not a Conventional Commit squash title with a pull request number: ` +
        commit.subject
    );
  }
  let summary = summaryFromBody(commit.body);
  // This immutable squash commit predates enforcement of the Markdown heading.
  // Keep the recovery exception bound to its exact object so later commits stay strict.
  if (!summary && PLAIN_SUMMARY_COMMIT_EXCEPTIONS.has(commit.hash)) {
    summary = plainSummaryFromBody(commit.body);
  }
  if (!summary) throw new Error(`${commit.hash} has no release summary`);
  const breakingFooter = hasBreakingFooter(commit.body);
  const category =
    match.groups.breaking || breakingFooter
      ? 'breaking'
      : CATEGORY_KEYS.has(match.groups.type)
        ? match.groups.type
        : 'maintenance';
  return {
    category,
    pull: match.groups.pull,
    summary,
    title: match.groups.title,
  };
}

function sentenceTitle(value) {
  return `${value.charAt(0).toUpperCase()}${value.slice(1)}`;
}

function renderReleaseNotes({ version, previousTag, commits }) {
  const releaseTag = `v${version}`;
  const groups = new Map(CATEGORIES.map(([key]) => [key, []]));
  for (const commit of commits) {
    const entry = parseReleaseCommit(commit);
    groups.get(entry.category).push(entry);
  }
  const lines = [
    `Changes since [${previousTag}](${REPOSITORY_URL}/compare/${previousTag}...${releaseTag}).`,
    '',
  ];
  for (const [category, heading] of CATEGORIES) {
    const entries = groups.get(category);
    if (entries.length === 0) continue;
    lines.push(`## ${heading}`, '');
    for (const entry of entries) {
      lines.push(
        `### ${sentenceTitle(entry.title)} ` +
          `([#${entry.pull}](${REPOSITORY_URL}/pull/${entry.pull}))`,
        '',
        entry.summary,
        ''
      );
    }
  }
  lines.push(
    '## Upgrade',
    '',
    '```bash',
    `npm install -g @the-open-engine-company/zeroshot@${version}`,
    '```',
    ''
  );
  return lines.join('\n');
}

function generateReleaseNotes({ repository, version, commit }) {
  if (!RELEASE_VERSION.test(version)) throw new Error(`invalid release version: ${version}`);
  if (!RELEASE_COMMIT.test(commit)) throw new Error(`invalid release commit: ${commit}`);
  const resolvedCommit = runGit(repository, ['rev-parse', `${commit}^{commit}`]);
  if (resolvedCommit !== commit)
    throw new Error(`release commit did not resolve exactly: ${commit}`);
  const releaseTag = `v${version}`;
  const previousTag = previousCanonicalTag(repository, commit, releaseTag);
  const commits = releaseCommits(repository, previousTag, commit);
  return {
    commits: commits.length,
    notes: renderReleaseNotes({ version, previousTag, commits }),
    previousTag,
  };
}

function run() {
  const options = parseArguments(process.argv.slice(2));
  const generated = generateReleaseNotes(options);
  fs.mkdirSync(path.dirname(options.output), { recursive: true });
  fs.writeFileSync(options.output, generated.notes, { encoding: 'utf8', flag: 'wx' });
  process.stdout.write(
    `generated release notes for ${generated.commits} merged pull requests ` +
      `since ${generated.previousTag}\n`
  );
}

function fail(error) {
  process.stderr.write(`${error.message}\n`);
  process.exitCode = 1;
}

if (require.main === module) {
  try {
    run();
  } catch (error) {
    fail(error);
  }
}

module.exports = {
  generateReleaseNotes,
  parseReleaseCommit,
  renderReleaseNotes,
  summaryFromBody,
};
