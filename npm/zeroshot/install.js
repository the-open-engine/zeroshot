#!/usr/bin/env node
'use strict';

const { install } = require('./lib/install');
const { requireCompleteSkillInstall } = require('./lib/skills');

install({ onSkillResults: requireCompleteSkillInstall }).catch((error) => {
  process.stderr.write(`zeroshot install failed: ${error.message}\n`);
  process.exitCode = 1;
});
