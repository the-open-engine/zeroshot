#!/usr/bin/env node

const { execFileSync } = require('child_process');

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

function main() {
  const name = packageName();
  const latest = npmLatest(name);
  const expectedTag = `v${latest}`;
  const headTags = tagsPointingAtHead();

  console.log(`npm latest for ${name}: ${latest}`);
  console.log(`tags on HEAD: ${headTags.join(', ') || '(none)'}`);

  if (!headTags.includes(expectedTag)) {
    throw new Error(`expected ${expectedTag} to point at HEAD after release`);
  }

  console.log(`Release publication verified: ${name}@${latest}`);
}

if (require.main === module) {
  try {
    main();
  } catch (error) {
    console.error(`Release publication check failed: ${error.message}`);
    process.exit(1);
  }
}

module.exports = {
  npmLatest,
  tagsPointingAtHead,
};
