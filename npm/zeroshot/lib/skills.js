'use strict';

const crypto = require('node:crypto');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const SKILL_NAME = 'zeroshot';
const MANAGED_MARKER = 'managed by @the-open-engine-company/zeroshot';
const MANAGED_PATTERN =
  /^<!-- managed by @the-open-engine-company\/zeroshot; sha256=([a-f0-9]{64}) -->\n/;
const TARGETS = Object.freeze([
  Object.freeze({ id: 'agents', label: 'Codex/GitHub Copilot' }),
  Object.freeze({ id: 'claude', label: 'Claude Code' }),
]);

function digest(value) {
  return crypto.createHash('sha256').update(value).digest('hex');
}

function frontmatterEnd(document) {
  if (!document.startsWith('---\n')) {
    throw new Error('SKILL_INVALID: SKILL.md needs YAML frontmatter');
  }
  const closing = document.indexOf('\n---\n', 4);
  if (closing === -1) throw new Error('SKILL_INVALID: SKILL.md frontmatter is not closed');
  return closing + 5;
}

function managedDocument(canonical) {
  const offset = frontmatterEnd(canonical);
  const marker = `<!-- ${MANAGED_MARKER}; sha256=${digest(canonical)} -->\n`;
  return `${canonical.slice(0, offset)}${marker}${canonical.slice(offset)}`;
}

function managedSource(document) {
  let offset;
  try {
    offset = frontmatterEnd(document);
  } catch {
    return null;
  }
  const match = MANAGED_PATTERN.exec(document.slice(offset));
  if (!match) return null;
  const source = `${document.slice(0, offset)}${document.slice(offset + match[0].length)}`;
  return digest(source) === match[1] ? source : null;
}

function locationResults(homeDirectory, environment, currentDirectory) {
  if (!homeDirectory || !path.isAbsolute(homeDirectory)) {
    return TARGETS.map((target) => ({
      ...target,
      status: 'failed',
      message: 'user home is unavailable or not absolute',
    }));
  }
  const shared = path.join(homeDirectory, '.agents', 'skills', SKILL_NAME);
  const configuredClaude = environment.CLAUDE_CONFIG_DIR;
  const claudeRoot = configuredClaude
    ? path.resolve(currentDirectory, configuredClaude)
    : path.join(homeDirectory, '.claude');
  return [
    { ...TARGETS[0], directory: shared },
    { ...TARGETS[1], directory: path.join(claudeRoot, 'skills', SKILL_NAME) },
  ];
}

function lstatIfPresent(filename) {
  try {
    return fs.lstatSync(filename);
  } catch (error) {
    if (error.code === 'ENOENT') return null;
    throw error;
  }
}

function atomicWrite(filename, contents) {
  const temporary = path.join(
    path.dirname(filename),
    `.SKILL.md.${process.pid}.${crypto.randomBytes(6).toString('hex')}.tmp`
  );
  try {
    fs.writeFileSync(temporary, contents, { encoding: 'utf8', flag: 'wx', mode: 0o644 });
    fs.renameSync(temporary, filename);
  } finally {
    fs.rmSync(temporary, { force: true });
  }
}

function inspectExisting(filename, canonical, managed) {
  const existingStat = lstatIfPresent(filename);
  if (!existingStat) return { status: 'installed' };
  if (!existingStat.isFile() || existingStat.isSymbolicLink()) {
    return { status: 'conflict', message: 'SKILL.md is not a regular file' };
  }
  const existing = fs.readFileSync(filename, 'utf8');
  if (existing === managed) return { status: 'unchanged' };
  if (existing === canonical || managedSource(existing) !== null) return { status: 'updated' };
  return {
    status: 'conflict',
    message: 'existing skill is not an unmodified Zeroshot-managed copy',
  };
}

function installAt(location, canonical, managed) {
  if (location.status === 'failed') return location;
  const filename = path.join(location.directory, 'SKILL.md');
  try {
    fs.mkdirSync(location.directory, { recursive: true });
    const directory = fs.lstatSync(location.directory);
    if (!directory.isDirectory() || directory.isSymbolicLink()) {
      return {
        ...location,
        path: filename,
        status: 'conflict',
        message: 'skill directory is not a regular directory',
      };
    }
    const inspection = inspectExisting(filename, canonical, managed);
    if (inspection.status === 'unchanged' || inspection.status === 'conflict') {
      return { ...location, path: filename, ...inspection };
    }
    atomicWrite(filename, managed);
    return { ...location, path: filename, status: inspection.status };
  } catch (error) {
    return { ...location, path: filename, status: 'failed', message: error.message };
  }
}

function installCurrentDirectory(options, environment) {
  return options.currentDirectory ?? environment.INIT_CWD ?? process.cwd();
}

function installSkills(options = {}) {
  const packageRoot = options.packageRoot || path.resolve(__dirname, '..');
  const source = path.join(packageRoot, 'skills', SKILL_NAME, 'SKILL.md');
  const canonical = fs.readFileSync(source, 'utf8');
  const managed = managedDocument(canonical);
  const homeDirectory = options.homeDirectory ?? os.homedir();
  const environment = options.environment ?? process.env;
  const currentDirectory = installCurrentDirectory(options, environment);
  const uid = options.uid ?? (typeof process.getuid === 'function' ? process.getuid() : undefined);
  const locations = locationResults(homeDirectory, environment, currentDirectory);
  if (uid === 0 && environment.SUDO_USER && environment.SUDO_USER !== 'root') {
    return locations.map((location) => ({
      ...location,
      status: 'failed',
      message:
        'refusing to create user skills from a sudo npm install; use a user-owned npm prefix',
    }));
  }
  return locations.map((location) => installAt(location, canonical, managed));
}

function humanList(values) {
  if (values.length < 2) return values[0] || '';
  if (values.length === 2) return `${values[0]} and ${values[1]}`;
  return `${values.slice(0, -1).join(', ')}, and ${values.at(-1)}`;
}

function reportSkillResults(results, output = process.stdout, errorOutput = process.stderr) {
  const ready = results.filter(({ status }) =>
    ['installed', 'updated', 'unchanged'].includes(status)
  );
  const incomplete = results.filter(
    ({ status }) => !['installed', 'updated', 'unchanged'].includes(status)
  );
  if (ready.length > 0) {
    output.write(`zeroshot skill ready for ${humanList(ready.map(({ label }) => label))}\n`);
  }
  for (const result of incomplete) {
    const location = result.path ? `${result.path}: ` : '';
    errorOutput.write(
      `zeroshot skill not installed for ${result.label}: ${location}${result.message}\n`
    );
  }
  if (incomplete.length > 0) {
    errorOutput.write(
      'Re-run npm install with lifecycle scripts enabled after resolving these paths.\n'
    );
  }
  return incomplete.length === 0;
}

function requireCompleteSkillInstall(results, output, errorOutput) {
  if (!reportSkillResults(results, output, errorOutput)) {
    throw new Error('SKILL_INSTALL_INCOMPLETE: Zeroshot skill installation was incomplete');
  }
}

module.exports = {
  installSkills,
  managedDocument,
  managedSource,
  reportSkillResults,
  requireCompleteSkillInstall,
};
