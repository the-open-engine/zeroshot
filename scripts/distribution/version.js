'use strict';

const fs = require('fs');
const path = require('path');
const { normalizeVersion, repositoryRoot } = require('./artifacts');

const VERSION_ERROR = 'ZEROSHOT_VERSION_MISMATCH';
const STAGED_LOCK_DEPENDENCIES = Object.freeze(['windows-sys']);

function cargoVersion(cargoToml) {
  const packageSection = cargoToml.match(/\[package\]([\s\S]*?)(?:\n\[|$)/);
  const version = packageSection && packageSection[1].match(/^version\s*=\s*"([^"]+)"\s*$/m);
  if (!version) throw new Error('zeroshot/Cargo.toml has no package version');
  return version[1];
}

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

module.exports = {
  VERSION_ERROR,
  cargoVersion,
  checkVersionCoupling,
  stageVersion,
};
