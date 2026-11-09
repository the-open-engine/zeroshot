'use strict';

const { execFileSync } = require('node:child_process');
const { createHash } = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');
const { crc32 } = require('node:zlib');
const { SKILL_NAMES } = require('../npm/zeroshot/lib/skills');

const repositoryRoot = path.resolve(__dirname, '..');
const metadataPath = 'plugins/metadata.json';
const packagePaths = ['plugins/zeroshot-openai', 'plugins/zeroshot-claude'];
const catalogPaths = ['.agents/plugins/marketplace.json', '.claude-plugin/marketplace.json'];
const endpoint = 'https://api.app.zeroshot.sh/mcp';
const remoteSkills = ['zeroshot', 'zeroshot-plan', 'zeroshot-graph-design'];
const json = (value) => Buffer.from(`${JSON.stringify(value, null, 2)}\n`);
const fail = (message) => {
  throw new Error(`PLUGIN_INVALID: ${message}`);
};

function noSymlinks(filename) {
  const absolute = path.resolve(filename);
  let current = path.parse(absolute).root;
  for (const component of absolute.slice(current.length).split(path.sep)) {
    current = path.join(current, component);
    let stat;
    try {
      stat = fs.lstatSync(current);
    } catch (error) {
      if (error.code === 'ENOENT') continue;
      throw error;
    }
    if (stat.isSymbolicLink()) fail(`symlink is not allowed: ${current}`);
  }
}

function readSource(root, relative) {
  if (!relative || relative.includes('\\') || path.posix.isAbsolute(relative)) {
    fail(`source path must be repository-relative: ${relative}`);
  }
  if (relative.split('/').some((part) => !part || part === '.' || part === '..')) {
    fail(`source path escapes its folder: ${relative}`);
  }
  const filename = path.join(root, relative);
  noSymlinks(filename);
  if (!fs.statSync(filename).isFile()) fail(`not a regular source file: ${relative}`);
  return fs.readFileSync(filename);
}

function versionParts(version) {
  if (typeof version !== 'string' || !/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(version)) {
    fail('version must be a stable X.Y.Z release SemVer');
  }
  return version.split('.').map(BigInt);
}

function metadata(root) {
  const value = JSON.parse(readSource(root, metadataPath));
  versionParts(value.version);
  if (value.name !== 'zeroshot' || value.marketplace !== 'the-open-engine') {
    fail('plugin and marketplace identities are permanent');
  }
  for (const key of ['displayName', 'description', 'publisher']) {
    if (typeof value[key] !== 'string' || !value[key].trim()) fail(`missing metadata ${key}`);
  }
  for (const key of [
    'websiteURL',
    'supportURL',
    'privacyPolicyURL',
    'termsOfServiceURL',
    'documentationURL',
  ]) {
    const url = new URL(value[key]);
    if (url.protocol !== 'https:' || url.username || url.password) fail(`invalid public ${key}`);
  }
  return value;
}

function iconBytes(root, source) {
  const bytes = readSource(root, source);
  if (bytes.length < 24 || !bytes.subarray(0, 8).equals(Buffer.from('89504e470d0a1a0a', 'hex'))) {
    fail('the existing brand icon must be a PNG');
  }
  const width = bytes.readUInt32BE(16);
  if (width !== bytes.readUInt32BE(20) || width < 512 || width > 2048 || bytes.length > 2_000_000) {
    fail('brand icon must be square, 512–2048 pixels, and under 2 MB');
  }
  return bytes;
}

function openaiManifest(value) {
  return {
    name: value.name,
    version: value.version,
    description: value.description,
    author: { name: value.publisher, url: value.websiteURL },
    homepage: value.websiteURL,
    repository: 'https://github.com/the-open-engine/zeroshot',
    license: 'MIT',
    skills: './skills/',
    mcpServers: './.mcp.json',
    interface: {
      displayName: value.displayName,
      shortDescription: 'Plan and run software work',
      longDescription: value.description,
      developerName: value.publisher,
      category: 'Developer Tools',
      capabilities: ['Read', 'Write'],
      websiteURL: value.websiteURL,
      supportURL: value.supportURL,
      privacyPolicyURL: value.privacyPolicyURL,
      termsOfServiceURL: value.termsOfServiceURL,
      composerIcon: './assets/icon.png',
      logo: './assets/icon.png',
    },
  };
}

function claudeManifest(value) {
  return {
    name: value.name,
    displayName: value.displayName,
    version: value.version,
    description: value.description,
    author: { name: value.publisher, url: value.websiteURL },
    homepage: value.websiteURL,
    repository: 'https://github.com/the-open-engine/zeroshot',
    license: 'MIT',
    icon: './assets/icon.png',
    privacyPolicyUrl: value.privacyPolicyURL,
  };
}

function readme(value, client) {
  const installation =
    client === 'openai'
      ? `codex plugin marketplace add the-open-engine/zeroshot\ncodex plugin add ${value.name}@${value.marketplace}`
      : `claude plugin marketplace add the-open-engine/zeroshot\nclaude plugin install ${value.name}@${value.marketplace}`;
  const browserSetup =
    client === 'openai'
      ? 'ChatGPT testing uses a local or workspace plugin source; public directory listings\nrequire separate approval.\n'
      : 'In Claude chat or Cowork, add this repository under Customize > Plugins > Add marketplace,\nthen connect the bundled connector from its Connectors tab.\n';
  return Buffer.from(
    `# ${value.displayName}\n\n${value.description}\n\n` +
      `This package contains the shared run, planning, and graph-design skills and the hosted\n` +
      `Zeroshot MCP connection. The Zeroshot CLI is optional for these Platform workflows.\n\n` +
      `## Install and connect\n\n\`\`\`sh\n${installation}\n\`\`\`\n\n` +
      `Start a new session after installation and authorize the Zeroshot MCP connection.\n` +
      `${browserSetup}\n` +
      `Ask to plan a change, design a reusable workflow, or run an existing Platform profile.\n` +
      `Execution requires your request and an exact repository commit. Configure provider\n` +
      `credentials in Platform; never paste secrets into a conversation. Local history\n` +
      `initialization and stacks require the CLI and are outside this package's MCP scope.\n\n` +
      `See the [installation and upgrade guide](${value.documentationURL}) for client setup.\n` +
      `The package connects to ${endpoint}; Platform processes the task data sent through\n` +
      `that connection under its [privacy notice](${value.privacyPolicyURL}) and\n` +
      `[terms](${value.termsOfServiceURL}). [Support](${value.supportURL}).\n\n` +
      `Generated from the canonical skills in this repository. Edit their sources rather\n` +
      `than these generated copies. Update the shared plugin version when package content changes.\n`
  );
}

function addPackageFiles(files, packagePath, index, { value, icon, license, skills }) {
  const client = index === 0 ? 'openai' : 'claude';
  const manifest = index === 0 ? openaiManifest(value) : claudeManifest(value);
  files.set(
    `${packagePath}/${index === 0 ? '.codex-plugin/plugin.json' : '.claude-plugin/plugin.json'}`,
    json(manifest)
  );
  const server = { type: 'http', url: endpoint };
  if (index === 0)
    server.extensions = {
      'com.openai': { auth: { type: 'oauth', client: { mode: 'dcr' } } },
    };
  files.set(`${packagePath}/.mcp.json`, json({ mcpServers: { zeroshot: server } }));
  files.set(`${packagePath}/README.md`, readme(value, client));
  files.set(`${packagePath}/LICENSE`, license);
  files.set(`${packagePath}/assets/icon.png`, icon);
  for (const [name, bytes] of skills) files.set(`${packagePath}/skills/${name}/SKILL.md`, bytes);
}

function buildFiles(root = repositoryRoot) {
  const value = metadata(root);
  if (remoteSkills.some((name) => !SKILL_NAMES.includes(name))) {
    fail('remote bundle names must belong to the canonical skill catalog');
  }
  const files = new Map();
  const assets = {
    value,
    icon: iconBytes(root, value.icon),
    license: readSource(root, 'LICENSE'),
    skills: remoteSkills.map((name) => [
      name,
      readSource(root, `npm/zeroshot/skills/${name}/SKILL.md`),
    ]),
  };
  for (const [index, packagePath] of packagePaths.entries())
    addPackageFiles(files, packagePath, index, assets);
  const identity = { name: value.marketplace, owner: { name: value.publisher } };
  files.set(
    catalogPaths[0],
    json({
      ...identity,
      interface: { displayName: value.publisher },
      plugins: [
        {
          name: value.name,
          source: { source: 'local', path: `./${packagePaths[0]}` },
          policy: { installation: 'AVAILABLE', authentication: 'ON_INSTALL' },
          category: 'Developer Tools',
        },
      ],
    })
  );
  files.set(
    catalogPaths[1],
    json({
      ...identity,
      description: 'Zeroshot skills and its hosted Platform connection.',
      plugins: [
        { name: value.name, source: `./${packagePaths[1]}`, description: value.description },
      ],
    })
  );
  for (const [filename, bytes] of files) {
    if (!filename.endsWith('.png') && bytes.length > 256 * 1024) {
      fail(`package text file exceeds the directory inspection limit: ${filename}`);
    }
  }
  return files;
}

function walkFiles(root, relative, result = new Map()) {
  const filename = path.join(root, relative);
  noSymlinks(filename);
  const stat = fs.statSync(filename);
  if (stat.isDirectory()) {
    for (const name of fs.readdirSync(filename).sort())
      walkFiles(root, `${relative}/${name}`, result);
  } else if (stat.isFile()) {
    if (stat.size > 5 * 1024 * 1024) fail(`package file is too large: ${relative}`);
    result.set(relative, fs.readFileSync(filename));
  } else fail(`package entry is not a regular file: ${relative}`);
  return result;
}

function check(root = repositoryRoot, base) {
  const expected = buildFiles(root);
  const actual = new Map();
  for (const directory of packagePaths) walkFiles(root, directory, actual);
  for (const catalog of catalogPaths) actual.set(catalog, readSource(root, catalog));
  if (actual.size !== expected.size)
    fail('generated package files are missing or unexpected; run plugins:generate');
  for (const [filename, bytes] of expected) {
    if (!actual.get(filename)?.equals(bytes))
      fail(`generated drift: ${filename}; run plugins:generate`);
  }
  if (base !== undefined) checkVersion(root, base, expected);
  return expected;
}

function generate(root = repositoryRoot) {
  const expected = buildFiles(root);
  for (const directory of packagePaths) {
    noSymlinks(path.join(root, directory));
    if (fs.existsSync(path.join(root, directory))) walkFiles(root, directory);
  }
  for (const filename of expected.keys()) noSymlinks(path.join(root, filename));
  for (const directory of packagePaths)
    fs.rmSync(path.join(root, directory), { recursive: true, force: true });
  for (const [filename, bytes] of expected) {
    fs.mkdirSync(path.dirname(path.join(root, filename)), { recursive: true });
    fs.writeFileSync(path.join(root, filename), bytes);
  }
  return expected;
}

function gitEnvironment() {
  const env = { ...process.env };
  // Hooks export repository-local context; -C must select the requested repository.
  const variables = execFileSync('git', ['rev-parse', '--local-env-vars'], {
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  for (const name of variables.trim().split('\n')) delete env[name];
  return env;
}

function git(root, args) {
  try {
    return execFileSync('git', ['-C', root, ...args], {
      env: gitEnvironment(),
      maxBuffer: 8 * 1024 * 1024,
      stdio: ['ignore', 'pipe', 'pipe'],
    });
  } catch {
    fail('base history is unavailable; fetch and pass the exact base commit');
  }
}

function normalized(filename, bytes) {
  if (
    filename === `${packagePaths[0]}/.codex-plugin/plugin.json` ||
    filename === `${packagePaths[1]}/.claude-plugin/plugin.json`
  ) {
    const value = JSON.parse(bytes);
    delete value.version;
    return json(value);
  }
  return bytes;
}

function compareVersions(previous, current) {
  const before = versionParts(previous);
  const after = versionParts(current);
  for (let index = 0; index < 3; index++) {
    if (after[index] !== before[index]) return after[index] > before[index] ? 1 : -1;
  }
  return 0;
}

function checkVersion(root, base, current = check(root)) {
  if (!/^(?:[a-f0-9]{40}|[a-f0-9]{64})$/.test(base)) fail('base must be an exact full commit SHA');
  if (git(root, ['cat-file', '-t', base]).toString().trim() !== 'commit')
    fail('base must name a commit');
  const names = git(root, [
    'ls-tree',
    '-r',
    '-z',
    '--name-only',
    base,
    '--',
    metadataPath,
    ...packagePaths,
    ...catalogPaths,
  ])
    .toString()
    .split('\0')
    .filter(Boolean);
  if (!names.includes(metadataPath)) {
    if (names.length > 0)
      fail('base has plugin outputs without metadata; cannot prove first introduction');
    return;
  }
  const previous = JSON.parse(git(root, ['show', `${base}:${metadataPath}`]));
  const comparison = compareVersions(previous.version, metadata(root).version);
  if (comparison < 0) fail('plugin version must not decrease');
  const priorFiles = names.filter((filename) => filename !== metadataPath);
  const changed =
    priorFiles.length !== current.size ||
    priorFiles.some((filename) => {
      const prior = git(root, ['show', `${base}:${filename}`]);
      return (
        !current.has(filename) ||
        !normalized(filename, prior).equals(normalized(filename, current.get(filename)))
      );
    });
  if (changed && comparison === 0)
    fail('package content changed without a strictly greater plugin version');
}

function archive(entries) {
  const local = [];
  const central = [];
  let offset = 0;
  for (const [name, bytes] of entries) {
    const filename = Buffer.from(name);
    const header = Buffer.alloc(30);
    header.writeUInt32LE(0x04034b50, 0);
    header.writeUInt16LE(20, 4);
    header.writeUInt16LE(0x0800, 6);
    header.writeUInt16LE(0x21, 12);
    header.writeUInt32LE(crc32(bytes), 14);
    header.writeUInt32LE(bytes.length, 18);
    header.writeUInt32LE(bytes.length, 22);
    header.writeUInt16LE(filename.length, 26);
    const directory = Buffer.alloc(46);
    directory.writeUInt32LE(0x02014b50, 0);
    directory.writeUInt16LE(0x0314, 4);
    header.copy(directory, 6, 4, 30);
    directory.writeUInt32LE((0o100644 << 16) >>> 0, 38);
    directory.writeUInt32LE(offset, 42);
    central.push(directory, filename);
    local.push(header, filename, bytes);
    offset += header.length + filename.length + bytes.length;
  }
  const directory = Buffer.concat(central);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(entries.length, 8);
  end.writeUInt16LE(entries.length, 10);
  end.writeUInt32LE(directory.length, 12);
  end.writeUInt32LE(offset, 16);
  return Buffer.concat([...local, directory, end]);
}

function packagePlugins(root = repositoryRoot, output = path.join(root, '.tmp/plugins')) {
  const files = check(root);
  const out = path.resolve(output);
  noSymlinks(out);
  for (const protectedPath of ['plugins', '.agents', '.claude-plugin', 'npm/zeroshot/skills']) {
    const relative = path.relative(path.join(root, protectedPath), out);
    const outside =
      relative === '..' || relative.startsWith(`..${path.sep}`) || path.isAbsolute(relative);
    if (!outside) fail('archive output overlaps plugin sources');
  }
  fs.mkdirSync(out, { recursive: true });
  const version = metadata(root).version;
  return packagePaths.map((directory) => {
    const entries = [...files]
      .filter(([name]) => name.startsWith(`${directory}/`))
      .map(([name, bytes]) => [name.slice(directory.length + 1), bytes])
      .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
    const bytes = archive(entries);
    const filename = path.join(out, `${path.basename(directory)}-${version}.zip`);
    noSymlinks(filename);
    fs.writeFileSync(filename, bytes);
    return { path: filename, sha256: createHash('sha256').update(bytes).digest('hex') };
  });
}

const usage = 'usage: plugins.js generate | check [--base SHA] | package [--out DIR]';

function optionValue(options, flag) {
  if (options.length === 0) return undefined;
  if (options.length !== 2 || options[0] !== flag) fail(usage);
  return options[1];
}

function main(args) {
  const [command, ...options] = args;
  switch (command) {
    case 'generate':
      if (options.length !== 0) fail(usage);
      generate();
      break;
    case 'check':
      check(repositoryRoot, optionValue(options, '--base'));
      break;
    case 'package':
      process.stdout.write(
        `${JSON.stringify(packagePlugins(repositoryRoot, optionValue(options, '--out')), null, 2)}\n`
      );
      break;
    default:
      fail(usage);
  }
}

module.exports = { buildFiles, check, checkVersion, generate, packagePlugins };
if (require.main === module) main(process.argv.slice(2));
