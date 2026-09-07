'use strict';

const fs = require('fs');
const path = require('path');
const jsYaml = require('js-yaml');
const { repositoryRoot, targets } = require('./artifacts');

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

module.exports = { checkRepository };
