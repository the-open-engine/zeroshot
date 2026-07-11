#!/usr/bin/env node

const { execFileSync } = require('child_process');

const DEFAULT_ATTEMPTS = 24;
const DEFAULT_DELAY_MS = 5000;

function run(command, args) {
  return execFileSync(command, args, { encoding: 'utf8' }).trim();
}

function packageName() {
  return require('../package.json').name;
}

function npmLatest(name) {
  return JSON.parse(run('npm', ['view', name, 'dist-tags.latest', '--json']));
}

function tagsPointingAtHead() {
  run('git', ['fetch', '--tags', '--force']);
  return run('git', ['tag', '--points-at', 'HEAD', '--list', 'v[0-9]*'])
    .split(/\r?\n/)
    .map((tag) => tag.trim())
    .filter(Boolean);
}

function releaseTagParts(tag) {
  const match = tag.match(/^v(\d+)\.(\d+)\.(\d+)$/);
  if (!match) return null;
  return match.slice(1).map((part) => Number(part));
}

function compareReleaseTags(left, right) {
  const leftParts = releaseTagParts(left);
  const rightParts = releaseTagParts(right);
  for (let index = 0; index < leftParts.length; index += 1) {
    if (leftParts[index] !== rightParts[index]) return leftParts[index] - rightParts[index];
  }
  return 0;
}

function latestReleaseTag(tags) {
  const releaseTags = tags.filter((tag) => releaseTagParts(tag));
  releaseTags.sort(compareReleaseTags);
  return releaseTags.at(-1) || null;
}

function sleep(ms) {
  return new Promise((resolve) => {
    setTimeout(resolve, ms);
  });
}

async function waitForNpmLatest(name, expectedVersion, options = {}) {
  const attempts =
    options.attempts || Number(process.env.RELEASE_ASSERT_ATTEMPTS || DEFAULT_ATTEMPTS);
  const delayMs =
    options.delayMs || Number(process.env.RELEASE_ASSERT_DELAY_MS || DEFAULT_DELAY_MS);

  let latest = null;
  for (let attempt = 1; attempt <= attempts; attempt += 1) {
    latest = npmLatest(name);
    if (latest === expectedVersion) return latest;

    if (attempt < attempts) {
      console.log(
        `npm latest for ${name} is ${latest}; waiting for ${expectedVersion} (${attempt}/${attempts})`
      );
      await sleep(delayMs);
    }
  }

  throw new Error(`expected npm latest for ${name} to be ${expectedVersion}, got ${latest}`);
}

async function main() {
  const name = packageName();
  const headTags = tagsPointingAtHead();
  const expectedTag = latestReleaseTag(headTags);

  if (!expectedTag) {
    throw new Error('expected a vX.Y.Z release tag to point at HEAD after release');
  }

  console.log(`tags on HEAD: ${headTags.join(', ') || '(none)'}`);
  const expectedVersion = expectedTag.slice(1);
  const latest = await waitForNpmLatest(name, expectedVersion);

  console.log(`npm latest for ${name}: ${latest}`);

  console.log(`Release publication verified: ${name}@${latest}`);
}

if (require.main === module) {
  main().catch((error) => {
    console.error(`Release publication check failed: ${error.message}`);
    process.exit(1);
  });
}

module.exports = {
  latestReleaseTag,
  npmLatest,
  tagsPointingAtHead,
  waitForNpmLatest,
};
