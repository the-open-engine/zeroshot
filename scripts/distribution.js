#!/usr/bin/env node
'use strict';

const crypto = require('crypto');
const childProcess = require('child_process');
const fs = require('fs');
const os = require('os');
const path = require('path');
const jsYaml = require('js-yaml');
const zlib = require('zlib');

const repositoryRoot = path.resolve(__dirname, '..');
const targetManifestPath = path.join(repositoryRoot, 'distribution', 'zeroshot-targets.json');
const targets = Object.freeze(JSON.parse(fs.readFileSync(targetManifestPath, 'utf8')));
const VERSION_ERROR = 'ZEROSHOT_VERSION_MISMATCH';
const RELEASE_TAG_PREFIX = 'v';
const MINIMUM_RELEASE_MAJOR = 8;
const COMMANDS = Object.freeze([
  'package',
  'manifest',
  'verify',
  'dry-run',
  'stage-version',
  'check-version',
  'check-repository',
  'print-version',
  'smoke',
  'smoke-archive',
  'publish-assets',
]);

function normalizeVersion(tag) {
  const version =
    typeof tag === 'string' && tag.startsWith(RELEASE_TAG_PREFIX) ? tag.slice(1) : tag;
  const match = typeof version === 'string' ? /^(\d+)\.(\d+)\.(\d+)$/.exec(version) : null;
  if (!match) {
    throw new Error(
      `invalid Zeroshot release version ${JSON.stringify(tag)}; expected X.Y.Z or ${RELEASE_TAG_PREFIX}X.Y.Z`
    );
  }
  if (Number(match[1]) < MINIMUM_RELEASE_MAJOR) {
    throw new Error(
      `invalid Zeroshot release version ${JSON.stringify(tag)}; major version must be ${MINIMUM_RELEASE_MAJOR} or newer`
    );
  }
  return version;
}

function releaseTag(version) {
  return `${RELEASE_TAG_PREFIX}${normalizeVersion(version)}`;
}

function archiveName(version, target) {
  return `zeroshot-v${normalizeVersion(version)}-${target}.tar.gz`;
}

function targetForHost(platform, arch) {
  const found = targets.find(
    (candidate) => candidate.platform === platform && candidate.arch === arch
  );
  if (!found) {
    throw new Error(
      `UNSUPPORTED_ZEROSHOT_HOST: no prebuilt binary for ${platform}/${arch}; supported hosts: ${targets
        .map((candidate) => `${candidate.platform}/${candidate.arch}`)
        .join(', ')}`
    );
  }
  return found;
}

function writeOctal(buffer, offset, length, value) {
  const encoded = value.toString(8).padStart(length - 1, '0') + '\0';
  buffer.write(encoded, offset, length, 'ascii');
}

function tarEntry(name, contents, mode = 0o755) {
  if (Buffer.byteLength(name) > 100) throw new Error(`archive entry name is too long: ${name}`);
  const header = Buffer.alloc(512);
  header.write(name, 0, 100, 'utf8');
  writeOctal(header, 100, 8, mode);
  writeOctal(header, 108, 8, 0);
  writeOctal(header, 116, 8, 0);
  writeOctal(header, 124, 12, contents.length);
  writeOctal(header, 136, 12, 0);
  header.fill(0x20, 148, 156);
  header[156] = '0'.charCodeAt(0);
  header.write('ustar\0', 257, 6, 'ascii');
  header.write('00', 263, 2, 'ascii');
  writeOctal(
    header,
    148,
    8,
    [...header].reduce((sum, byte) => sum + byte, 0)
  );
  const padding = Buffer.alloc((512 - (contents.length % 512)) % 512);
  return Buffer.concat([header, contents, padding]);
}

function createArchive(binary, executable) {
  const tar = Buffer.concat([tarEntry(executable, binary), Buffer.alloc(1024)]);
  return zlib.gzipSync(tar, { level: 9, mtime: 0 });
}

function extractExecutable(archive, expectedName) {
  const tar = zlib.gunzipSync(archive);
  const name = tar.subarray(0, 100).toString('utf8').replace(/\0.*$/, '');
  const sizeText = tar.subarray(124, 136).toString('ascii').replace(/\0.*$/, '').trim();
  if (name !== expectedName || !/^[0-7]+$/.test(sizeText)) {
    throw new Error(`ARCHIVE_INVALID: expected sole executable ${expectedName}`);
  }
  const size = Number.parseInt(sizeText, 8);
  const end = 512 + size;
  if (end > tar.length) throw new Error('ARCHIVE_INVALID: truncated executable');
  const nextHeader = 512 + Math.ceil(size / 512) * 512;
  if (!tar.subarray(nextHeader).every((byte) => byte === 0)) {
    throw new Error('ARCHIVE_INVALID: archive contains unexpected entries');
  }
  return Buffer.from(tar.subarray(512, end));
}

function sha256(contents) {
  return crypto.createHash('sha256').update(contents).digest('hex');
}

function parseChecksumManifest(text) {
  const checksums = new Map();
  for (const line of text.split(/\r?\n/)) {
    if (!line) continue;
    const match = /^([0-9a-f]{64}) {2}([^/\\]+)$/.exec(line);
    if (!match) throw new Error(`invalid SHA256SUMS line: ${line}`);
    if (checksums.has(match[2])) throw new Error(`duplicate SHA256SUMS entry: ${match[2]}`);
    checksums.set(match[2], match[1]);
  }
  return checksums;
}

function verifyChecksum(filename, contents, manifest) {
  const checksums = manifest instanceof Map ? manifest : parseChecksumManifest(manifest);
  const expected = checksums.get(filename);
  if (!expected) throw new Error(`CHECKSUM_MISSING: SHA256SUMS has no entry for ${filename}`);
  const actual = sha256(contents);
  if (actual !== expected) {
    throw new Error(`CHECKSUM_MISMATCH: ${filename} expected ${expected} but received ${actual}`);
  }
  return true;
}

function packageTarget({ target, version, binaryPath, outputDirectory }) {
  const declaration = targets.find((candidate) => candidate.target === target);
  if (!declaration) throw new Error(`undeclared Zeroshot release target: ${target}`);
  const binary = fs.readFileSync(binaryPath);
  const filename = archiveName(version, target);
  fs.mkdirSync(outputDirectory, { recursive: true });
  fs.writeFileSync(
    path.join(outputDirectory, filename),
    createArchive(binary, declaration.executable)
  );
  return filename;
}

function createManifest({ version, directory }) {
  const entries = targets.map(({ target }) => {
    const filename = archiveName(version, target);
    const contents = fs.readFileSync(path.join(directory, filename));
    return `${sha256(contents)}  ${filename}`;
  });
  const manifest = `${entries.join('\n')}\n`;
  fs.writeFileSync(path.join(directory, 'SHA256SUMS'), manifest);
  verifyDistribution({ version, directory });
  return manifest;
}

function verifyDistribution({ version, directory }) {
  const expected = targets.map(({ target }) => archiveName(version, target));
  const manifest = parseChecksumManifest(
    fs.readFileSync(path.join(directory, 'SHA256SUMS'), 'utf8')
  );
  if (manifest.size !== expected.length || expected.some((filename) => !manifest.has(filename))) {
    throw new Error(
      `DISTRIBUTION_INCOMPLETE: SHA256SUMS must contain exactly ${expected.length} declared archives`
    );
  }
  for (const declaration of targets) {
    const filename = archiveName(version, declaration.target);
    const archive = fs.readFileSync(path.join(directory, filename));
    verifyChecksum(filename, archive, manifest);
    extractExecutable(archive, declaration.executable);
  }
  return true;
}

function runGh(args) {
  return childProcess.execFileSync('gh', args, { encoding: 'utf8' });
}

function publishAssets({ tag, directory, invokeGh = runGh }) {
  const names = [...targets.map(({ target }) => archiveName(tag, target)), 'SHA256SUMS'];
  const localAssets = new Map(
    names.map((name) => [name, fs.readFileSync(path.join(directory, name))])
  );
  const release = JSON.parse(invokeGh(['release', 'view', tag, '--json', 'assets']));
  const existingNames = release.assets.map(({ name }) => name);
  if (new Set(existingNames).size !== existingNames.length) {
    throw new Error('RELEASE_ASSET_CONFLICT: GitHub Release contains duplicate asset names');
  }
  const unexpected = existingNames.filter((name) => !localAssets.has(name));
  if (unexpected.length > 0) {
    throw new Error(
      `RELEASE_ASSET_CONFLICT: GitHub Release contains unexpected assets: ${unexpected.join(', ')}`
    );
  }

  const existingRequired = names.filter((name) => existingNames.includes(name));
  const temporary = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-assets-'));
  try {
    for (const name of existingRequired) {
      invokeGh(['release', 'download', tag, '--pattern', name, '--dir', temporary]);
      const published = fs.readFileSync(path.join(temporary, name));
      if (!published.equals(localAssets.get(name))) {
        throw new Error(`RELEASE_ASSET_CONFLICT: existing ${name} differs from verified artifact`);
      }
    }
  } finally {
    fs.rmSync(temporary, { recursive: true, force: true });
  }

  const missing = names.filter((name) => !existingNames.includes(name));
  for (const name of missing) {
    invokeGh(['release', 'upload', tag, path.join(directory, name)]);
  }
  return { existing: existingRequired, uploaded: missing };
}

function cargoVersion(cargoToml) {
  const packageSection = cargoToml.match(/\[package\]([\s\S]*?)(?:\n\[|$)/);
  const version = packageSection && packageSection[1].match(/^version\s*=\s*"([^"]+)"\s*$/m);
  if (!version) throw new Error('zeroshot/Cargo.toml has no package version');
  return version[1];
}

const STAGED_LOCK_DEPENDENCIES = Object.freeze(['windows-sys']);

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

function cargoLockPackages(cargoLock, packageName) {
  const tables = [];
  const marker = /^\[{1,2}[^\r\n]+\]{1,2}\r?$/gm;
  for (let match = marker.exec(cargoLock); match; match = marker.exec(cargoLock)) {
    tables.push({ header: match[0].trim(), start: match.index });
  }
  return tables.flatMap((table, index) => {
    if (table.header !== '[[package]]') return [];
    const text = cargoLock.slice(table.start, tables[index + 1]?.start ?? cargoLock.length);
    const name = text.match(/^name = "([^"]+)"\r?$/m)?.[1];
    const version = text.match(/^version = "([^"]+)"\r?$/m)?.[1];
    if (name !== packageName || !version) return [];
    return [
      {
        start: table.start,
        text,
        version,
        source: text.match(/^source = "([^"]+)"\r?$/m)?.[1],
      },
    ];
  });
}

function workspaceLockPackage(cargoLock) {
  const candidates = cargoLockPackages(cargoLock, 'zeroshot').filter(
    (candidate) => candidate.source === undefined
  );
  if (candidates.length !== 1) {
    throw new Error(
      'ZEROSHOT_VERSION_STAGE_FAILED: Cargo.lock needs exactly one source-less zeroshot package'
    );
  }
  return candidates[0];
}

function workspaceDependencyRequirement(workspaceCargoToml, dependencyName) {
  const workspaceDependencies = workspaceCargoToml.match(
    /\[workspace\.dependencies\]([\s\S]*?)(?:\r?\n\[|$)/
  );
  const requirement =
    workspaceDependencies &&
    workspaceDependencies[1].match(
      new RegExp(`^${escapeRegExp(dependencyName)}\\s*=\\s*"([^"]+)"\\s*$`, 'm')
    );
  if (!requirement || !/^(\^|=)?\d+\.\d+\.\d+$/.test(requirement[1])) {
    throw new Error(
      `ZEROSHOT_VERSION_STAGE_FAILED: workspace dependency ${dependencyName} has an unsupported version requirement`
    );
  }
  return requirement[1];
}

function parseCargoVersion(version) {
  const match = /^(\d+)\.(\d+)\.(\d+)$/.exec(version);
  return match ? match.slice(1).map(Number) : undefined;
}

function compareCargoVersions(left, right) {
  for (let index = 0; index < left.length; index += 1) {
    if (left[index] !== right[index]) return left[index] - right[index];
  }
  return 0;
}

function cargoRequirementMatches(requirement, version) {
  const parsedVersion = parseCargoVersion(version);
  const match = /^(\^|=)?(\d+)\.(\d+)\.(\d+)$/.exec(requirement);
  if (!parsedVersion || !match) return false;
  const lower = match.slice(2).map(Number);
  if (match[1] === '=') return compareCargoVersions(parsedVersion, lower) === 0;
  const upper =
    lower[0] > 0
      ? [lower[0] + 1, 0, 0]
      : lower[1] > 0
        ? [0, lower[1] + 1, 0]
        : [0, 0, lower[2] + 1];
  return (
    compareCargoVersions(parsedVersion, lower) >= 0 &&
    compareCargoVersions(parsedVersion, upper) < 0
  );
}

function stagedLockDependencies(cargoLock, workspaceCargoToml) {
  return STAGED_LOCK_DEPENDENCIES.map((name) => {
    const requirement = workspaceDependencyRequirement(workspaceCargoToml, name);
    const candidates = cargoLockPackages(cargoLock, name);
    const satisfying = candidates.filter((candidate) =>
      cargoRequirementMatches(requirement, candidate.version)
    );
    if (satisfying.length !== 1) {
      const sourceAmbiguous =
        satisfying.length > 1 &&
        new Set(satisfying.map((candidate) => candidate.version)).size === 1;
      throw new Error(
        sourceAmbiguous
          ? `ZEROSHOT_VERSION_STAGE_FAILED: Cargo.lock ${name} ${satisfying[0].version} has ambiguous sources`
          : `ZEROSHOT_VERSION_STAGE_FAILED: Cargo.lock needs exactly one ${name} package satisfying ${requirement}`
      );
    }
    const selected = satisfying[0];
    return {
      name,
      requirement,
      version: selected.version,
      reference: candidates.length > 1 ? `${name} ${selected.version}` : name,
    };
  });
}

function stageCargoLock(cargoLock, version, workspaceCargoToml) {
  const targetPackage = workspaceLockPackage(cargoLock);
  let stagedPackage = targetPackage.text.replace(/^(version = ")[^"]+(")$/m, `$1${version}$2`);
  for (const dependency of stagedLockDependencies(cargoLock, workspaceCargoToml)) {
    const dependencyPattern = new RegExp(
      `^(\\s*")${escapeRegExp(dependency.name)}(?: [^"]+)?(",\\r?)$`,
      'm'
    );
    if (!dependencyPattern.test(stagedPackage)) {
      throw new Error(
        `ZEROSHOT_VERSION_STAGE_FAILED: Cargo.lock zeroshot entry has no ${dependency.name} dependency`
      );
    }
    stagedPackage = stagedPackage.replace(dependencyPattern, `$1${dependency.reference}$2`);
  }
  return (
    cargoLock.slice(0, targetPackage.start) +
    stagedPackage +
    cargoLock.slice(targetPackage.start + targetPackage.text.length)
  );
}

function verifyStagedCargoLock(cargoLock, version, workspaceCargoToml) {
  const targetPackage = workspaceLockPackage(cargoLock);
  if (targetPackage.version !== version) {
    throw new Error(
      `${VERSION_ERROR}: release tag version ${version} does not match ` +
        `Cargo.lock zeroshot version ${targetPackage.version}`
    );
  }
  for (const dependency of stagedLockDependencies(cargoLock, workspaceCargoToml)) {
    const dependencyPattern = new RegExp(`^\\s*"${escapeRegExp(dependency.reference)}",\\r?$`, 'm');
    if (!dependencyPattern.test(targetPackage.text)) {
      throw new Error(
        `${VERSION_ERROR}: Cargo.lock zeroshot dependency ${dependency.name} is not coupled to ${dependency.version}`
      );
    }
  }
}

function stageVersion(
  tag,
  cargoManifestPath = path.join(repositoryRoot, 'zeroshot', 'Cargo.toml'),
  cargoLockPath = path.join(repositoryRoot, 'Cargo.lock'),
  workspaceManifestPath = path.join(repositoryRoot, 'Cargo.toml')
) {
  const version = normalizeVersion(tag);
  const cargoToml = fs.readFileSync(cargoManifestPath, 'utf8');
  const currentVersion = cargoVersion(cargoToml);
  const stagedManifest = cargoToml.replace(
    /(\[package\][\s\S]*?^version\s*=\s*")[^"]+(")/m,
    `$1${version}$2`
  );
  if (cargoVersion(stagedManifest) !== version) {
    throw new Error('ZEROSHOT_VERSION_STAGE_FAILED: Cargo.toml package version was not updated');
  }

  const cargoLock = fs.readFileSync(cargoLockPath, 'utf8');
  const workspaceCargoToml = fs.readFileSync(workspaceManifestPath, 'utf8');
  const stagedLock = stageCargoLock(cargoLock, version, workspaceCargoToml);
  verifyStagedCargoLock(stagedLock, version, workspaceCargoToml);
  fs.writeFileSync(cargoManifestPath, stagedManifest);
  fs.writeFileSync(cargoLockPath, stagedLock);
  return { currentVersion, version };
}

function checkVersionCoupling(tag, cargoToml, cargoLock, workspaceCargoToml) {
  const useRepositoryFiles = cargoToml === undefined;
  const manifest =
    cargoToml ?? fs.readFileSync(path.join(repositoryRoot, 'zeroshot', 'Cargo.toml'), 'utf8');
  const releaseVersion = normalizeVersion(tag);
  const manifestVersion = cargoVersion(manifest);
  if (releaseVersion !== manifestVersion) {
    throw new Error(
      `${VERSION_ERROR}: release tag version ${releaseVersion} does not match ` +
        `zeroshot/Cargo.toml version ${manifestVersion}`
    );
  }
  const lock =
    cargoLock ??
    (useRepositoryFiles
      ? fs.readFileSync(path.join(repositoryRoot, 'Cargo.lock'), 'utf8')
      : undefined);
  if (lock !== undefined) {
    const workspace =
      workspaceCargoToml ?? fs.readFileSync(path.join(repositoryRoot, 'Cargo.toml'), 'utf8');
    verifyStagedCargoLock(lock, releaseVersion, workspace);
  }
  return releaseVersion;
}

function failIntegrity(message) {
  throw new Error(`ZEROSHOT_DISTRIBUTION_INTEGRITY: ${message}`);
}

function requireFragments(label, value, fragments) {
  const missing = fragments.filter((fragment) => !value.includes(fragment));
  if (missing.length > 0) failIntegrity(`${label} is missing: ${missing.join(', ')}`);
}

function repositoryContract(options) {
  return {
    workflow:
      options.workflow ??
      fs.readFileSync(path.join(repositoryRoot, '.github', 'workflows', 'release.yml'), 'utf8'),
    pythonWorkflow:
      options.pythonWorkflow ??
      fs.readFileSync(
        path.join(repositoryRoot, '.github', 'workflows', 'release-python.yml'),
        'utf8'
      ),
    shimTargets:
      options.shimTargets ??
      JSON.parse(
        fs.readFileSync(path.join(repositoryRoot, 'npm', 'zeroshot', 'targets.json'), 'utf8')
      ),
    packageManifest:
      options.packageManifest ??
      JSON.parse(fs.readFileSync(path.join(repositoryRoot, 'package.json'), 'utf8')),
    packageLock:
      options.packageLock ??
      JSON.parse(fs.readFileSync(path.join(repositoryRoot, 'package-lock.json'), 'utf8')),
  };
}

function parseReleaseWorkflow(workflow) {
  let document;
  try {
    document = jsYaml.load(workflow);
  } catch (error) {
    failIntegrity(`release workflow is invalid YAML: ${error.message}`);
  }
  if (document?.name !== 'Release Zeroshot' || !document.jobs) {
    failIntegrity('release.yml must define the canonical Release Zeroshot workflow');
  }
  return document;
}

function parsePythonReleaseWorkflow(workflow) {
  let document;
  try {
    document = jsYaml.load(workflow);
  } catch (error) {
    failIntegrity(`release-python workflow is invalid YAML: ${error.message}`);
  }
  if (document?.name !== 'Release Python SDK' || !document.jobs) {
    failIntegrity('release-python.yml must define the canonical Python SDK workflow');
  }
  return document;
}

function checkReleaseJobs(document) {
  const expectedJobs = [
    'plan',
    'binaries',
    'manifest',
    'image-input',
    'publish',
    'image-publish',
    'shim-input',
    'shim-publish',
    'python-sdk-release',
  ];
  for (const job of expectedJobs) {
    if (!document.jobs[job]) failIntegrity(`release workflow has no ${job} job`);
  }
}

function checkObsoleteWorkflowIdentities(workflow) {
  const forbidden = [
    'zeroshot-rust',
    'release-rust',
    'rust-release',
    'rust-binaries',
    'rust-manifest',
    'rust-image',
    'rust-publish',
    'rust-shim',
    'semantic-release',
  ];
  const stale = forbidden.filter((value) => workflow.includes(value));
  if (stale.length > 0) {
    failIntegrity(`release workflow retains obsolete identities: ${stale.join(', ')}`);
  }
}

function checkReleaseFragments(workflow) {
  requireFragments('release workflow', workflow, [
    'release_tag="v$RELEASE_VERSION"',
    '[[ "$major" -ge 8 ]]',
    'node scripts/distribution.js stage-version --tag "$RELEASE_TAG"',
    'node scripts/distribution.js check-version --tag "$RELEASE_TAG"',
    'cargo build --release --locked -p zeroshot --bin zeroshot --target ${{ matrix.target }}',
    'docker/zeroshot-target/Dockerfile',
    'ghcr.io/the-open-engine/zeroshot-target',
    'commit_ref="$ZEROSHOT_IMAGE:sha-$RELEASE_COMMIT"',
    'candidate_id="$(docker image inspect --format \'{{.Id}}\' "$candidate")"',
    '[[ "$existing_id" == "$candidate_id" ]]',
    'gh release edit "$RELEASE_TAG" --latest="$PUBLISH_LATEST"',
    'node scripts/distribution.js publish-assets --tag "$RELEASE_TAG" --dir release-assets',
    'npm publish --provenance --access public ./shim-release/*.tgz',
    'uses: ./.github/workflows/release-python.yml',
  ]);
}

function checkPythonReleaseFragments(workflow) {
  requireFragments('release-python workflow', workflow, [
    'https://pypi.org/pypi/the-open-engine-zeroshot/{version}/json',
    'SOURCE_DATE_EPOCH: ${{ needs.plan.outputs.source-date-epoch }}',
    'source_date_epoch="$(git show -s --format=%ct "$RELEASE_COMMIT")"',
    'if remote != local:',
    'gh release download "$SDK_TAG" --pattern "$name" --dir "$existing_dir"',
    '[[ "$local_sha" == "$remote_sha" ]]',
    'gh release edit "$SDK_TAG" --latest=false',
    "if: steps.pypi.outputs.exists == 'false'",
  ]);
  if (workflow.includes('skip-existing: true')) {
    failIntegrity('release-python workflow must verify existing files instead of skipping them');
  }
}

function normalizedReleaseTargets(declarations) {
  return declarations.map(({ platform, arch, target, executable }) => ({
    platform,
    arch,
    target,
    executable,
  }));
}

function checkTargetParity(shimTargets) {
  const normalizedShimTargets = normalizedReleaseTargets(shimTargets);
  const normalizedTargets = normalizedReleaseTargets(targets);
  if (JSON.stringify(normalizedShimTargets) !== JSON.stringify(normalizedTargets)) {
    failIntegrity('npm shim targets must exactly match the release target manifest');
  }
}

function checkToolingManifest(packageManifest, packageLock) {
  if (
    packageManifest.name !== '@the-open-engine-company/zeroshot-repository' ||
    packageManifest.private !== true
  ) {
    failIntegrity('root package must remain private and distinct from the public npm shim');
  }
  if (packageManifest.dependencies?.['js-yaml'] === undefined) {
    failIntegrity('scripts/distribution.js requires a direct js-yaml dependency');
  }
  if (packageLock.packages?.['']?.name !== packageManifest.name) {
    failIntegrity('package-lock root identity must match package.json');
  }
  const lockedYaml = packageLock.packages?.['node_modules/js-yaml'];
  if (!lockedYaml?.resolved || !/^sha512-[A-Za-z0-9+/=]+$/.test(lockedYaml.integrity || '')) {
    failIntegrity('package-lock must integrity-pin js-yaml');
  }
}

function checkRepository(options = {}) {
  const contract = repositoryContract(options);
  const document = parseReleaseWorkflow(contract.workflow);
  parsePythonReleaseWorkflow(contract.pythonWorkflow);
  checkReleaseJobs(document);
  checkObsoleteWorkflowIdentities(contract.workflow);
  checkReleaseFragments(contract.workflow);
  checkPythonReleaseFragments(contract.pythonWorkflow);
  checkTargetParity(contract.shimTargets);
  checkToolingManifest(contract.packageManifest, contract.packageLock);
  return true;
}

function argument(name) {
  const index = process.argv.indexOf(`--${name}`);
  if (index === -1 || !process.argv[index + 1]) throw new Error(`missing --${name}`);
  return process.argv[index + 1];
}

function runPackageCommand() {
  const filename = packageTarget({
    target: argument('target'),
    version: argument('version'),
    binaryPath: argument('binary'),
    outputDirectory: argument('out'),
  });
  process.stdout.write(`${filename}\n`);
}

function runManifestCommand() {
  createManifest({ version: argument('version'), directory: argument('dir') });
  process.stdout.write(`verified ${targets.length} archives and SHA256SUMS\n`);
}

function runVerifyCommand() {
  verifyDistribution({ version: argument('version'), directory: argument('dir') });
  process.stdout.write(`verified ${targets.length} existing archives and SHA256SUMS\n`);
}

function runDryRunCommand() {
  const version = argument('version');
  const binaryPath = argument('binary');
  const outputDirectory = argument('out');
  for (const { target } of targets) {
    packageTarget({ target, version, binaryPath, outputDirectory });
  }
  createManifest({ version, directory: outputDirectory });
  process.stdout.write(`dry-run produced and verified ${targets.length} archives\n`);
}

function runStageVersionCommand() {
  const staged = stageVersion(argument('tag'));
  process.stdout.write(
    `staged Zeroshot package version ${staged.currentVersion} -> ${staged.version}\n`
  );
}

function runCheckVersionCommand() {
  const version = checkVersionCoupling(argument('tag'));
  process.stdout.write(`Zeroshot package version matches release tag: ${version}\n`);
}

function runPrintVersionCommand() {
  const manifest = fs.readFileSync(path.join(repositoryRoot, 'zeroshot', 'Cargo.toml'), 'utf8');
  process.stdout.write(`${cargoVersion(manifest)}\n`);
}

function runSmokeCommand() {
  const binaryPath = path.resolve(argument('binary'));
  smokeExecutable(binaryPath, 'ZEROSHOT_BINARY_SMOKE_FAILED');
  process.stdout.write(`Zeroshot release executable exited 0: ${binaryPath}\n`);
}

function runSmokeArchiveCommand() {
  const target = argument('target');
  const declaration = targets.find((candidate) => candidate.target === target);
  if (!declaration) throw new Error(`undeclared Zeroshot release target: ${target}`);
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-smoke-'));
  const binaryPath = path.join(directory, declaration.executable);
  try {
    const archive = fs.readFileSync(argument('archive'));
    fs.writeFileSync(binaryPath, extractExecutable(archive, declaration.executable), {
      mode: 0o755,
    });
    smokeExecutable(binaryPath, 'ZEROSHOT_ARCHIVE_SMOKE_FAILED');
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
  process.stdout.write(`Zeroshot release archive executable exited 0: ${target}\n`);
}

function runPublishAssetsCommand() {
  const result = publishAssets({ tag: argument('tag'), directory: argument('dir') });
  process.stdout.write(
    `verified ${result.existing.length} existing assets and ` +
      `uploaded ${result.uploaded.length} missing assets\n`
  );
}

function runCheckRepositoryCommand() {
  checkRepository();
  process.stdout.write(`Zeroshot distribution declares ${targets.length} complete targets\n`);
}

const commandHandlers = new Map([
  ['package', runPackageCommand],
  ['manifest', runManifestCommand],
  ['verify', runVerifyCommand],
  ['dry-run', runDryRunCommand],
  ['stage-version', runStageVersionCommand],
  ['check-version', runCheckVersionCommand],
  ['print-version', runPrintVersionCommand],
  ['smoke', runSmokeCommand],
  ['smoke-archive', runSmokeArchiveCommand],
  ['publish-assets', runPublishAssetsCommand],
  ['check-repository', runCheckRepositoryCommand],
]);

function run() {
  const handler = commandHandlers.get(process.argv[2]);
  if (!handler) throw new Error(`usage: distribution.js <${COMMANDS.join('|')}>`);
  handler();
}

function smokeExecutable(
  binaryPath,
  failureCode,
  expectedVersion = cargoVersion(
    fs.readFileSync(path.join(repositoryRoot, 'zeroshot', 'Cargo.toml'), 'utf8')
  )
) {
  const result = childProcess.spawnSync(binaryPath, ['--version'], { encoding: 'utf8' });
  if (result.error) throw result.error;
  if (result.signal || result.status !== 0) {
    throw new Error(`${failureCode}: status=${result.status} signal=${result.signal || 'none'}`);
  }
  const expected = `zeroshot ${expectedVersion}\n`;
  if (result.stdout !== expected) {
    throw new Error(
      `${failureCode}: expected ${JSON.stringify(expected.trim())}, received ${JSON.stringify(result.stdout.trim())}`
    );
  }
}

if (require.main === module) {
  try {
    run();
  } catch (error) {
    process.stderr.write(`${error.message}\n`);
    process.exitCode = 1;
  }
}

module.exports = {
  MINIMUM_RELEASE_MAJOR,
  RELEASE_TAG_PREFIX,
  VERSION_ERROR,
  archiveName,
  checkRepository,
  checkVersionCoupling,
  createArchive,
  createManifest,
  extractExecutable,
  cargoVersion,
  normalizeVersion,
  packageTarget,
  parseChecksumManifest,
  publishAssets,
  releaseTag,
  sha256,
  smokeExecutable,
  stageVersion,
  targetForHost,
  targets,
  verifyChecksum,
  verifyDistribution,
};
