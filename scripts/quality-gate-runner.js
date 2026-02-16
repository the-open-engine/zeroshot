#!/usr/bin/env node

/**
 * Quality Gate Runner
 *
 * Reads .zeroshot-quality from CWD, runs the command, reports results.
 * Exit code matches the command's exit code (0 = pass, non-zero = fail).
 * If no .zeroshot-quality file exists, auto-passes with warning.
 */

const { execSync } = require('../src/lib/safe-exec');
const fs = require('fs');
const path = require('path');

const QUALITY_FILE = '.zeroshot-quality';

function run() {
  const qualityPath = path.join(process.cwd(), QUALITY_FILE);

  if (!fs.existsSync(qualityPath)) {
    const result = {
      command: null,
      exitCode: 0,
      stdout: `No ${QUALITY_FILE} file found — quality gate auto-passed`,
      stderr: '',
    };
    console.log(JSON.stringify(result));
    process.exit(0);
  }

  const command = fs.readFileSync(qualityPath, 'utf-8').trim();

  if (!command) {
    const result = {
      command: null,
      exitCode: 0,
      stdout: `${QUALITY_FILE} file is empty — quality gate auto-passed`,
      stderr: '',
    };
    console.log(JSON.stringify(result));
    process.exit(0);
  }

  let stdout = '';
  let stderr = '';
  let exitCode = 0;

  try {
    stdout = execSync(command, {
      encoding: 'utf-8',
      stdio: ['pipe', 'pipe', 'pipe'],
      timeout: 115000,
      cwd: process.cwd(),
    });
  } catch (error) {
    if (error.killed && error.signal) {
      exitCode = 124;
      stderr = `TIMEOUT: Command killed after timeout by signal ${error.signal}`;
      stdout = error.stdout || '';
    } else {
      exitCode = error.status || 1;
      stdout = error.stdout || '';
      stderr = error.stderr || '';
    }
  }

  const result = {
    command,
    exitCode,
    stdout: stdout || '',
    stderr: stderr || '',
  };

  console.log(JSON.stringify(result));
  process.exit(exitCode);
}

run();
