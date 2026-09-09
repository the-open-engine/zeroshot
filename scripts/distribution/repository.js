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
    docsWorkflow:
      options.docsWorkflow ??
      fs.readFileSync(path.join(repositoryRoot, '.github', 'workflows', 'docs.yml'), 'utf8'),
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

function parseDocsWorkflow(workflow) {
  let document;
  try {
    document = jsYaml.load(workflow);
  } catch (error) {
    failIntegrity(`docs workflow is invalid YAML: ${error.message}`);
  }
  if (document?.name !== 'Publish versioned documentation' || !document.jobs?.publish) {
    failIntegrity('docs.yml must define the versioned documentation publish workflow');
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
    'docs',
  ];
  for (const job of expectedJobs) {
    if (!document.jobs[job]) failIntegrity(`release workflow has no ${job} job`);
  }
}

function checkDocsReleaseJob(releaseDocument) {
  const job = releaseDocument.jobs?.docs;
  if (job?.uses !== './.github/workflows/docs.yml') {
    failIntegrity('release workflow must invoke docs.yml directly');
  }
  if (!job.needs?.includes('plan') || !job.needs?.includes('python-sdk-release')) {
    failIntegrity('release docs must wait for planning and Python SDK revision 1');
  }
  if (job.permissions?.contents !== 'write') {
    failIntegrity('release docs must receive contents write permission');
  }
  if (job.permissions?.pages !== 'write' || job.permissions?.['id-token'] !== 'write') {
    failIntegrity('release docs must receive Pages and OIDC write permissions');
  }
  const expectedInputs = {
    version: '${{ needs.plan.outputs.tag }}',
    release_commit: '${{ needs.plan.outputs.commit }}',
    stable: "${{ needs.plan.outputs.publish-latest == 'true' }}",
  };
  for (const [name, value] of Object.entries(expectedInputs)) {
    if (job.with?.[name] !== value) {
      failIntegrity(`release docs must pass ${name} from the canonical release plan`);
    }
  }
}

function checkDocsWorkflowInputs(docsDocument) {
  const trigger = docsDocument.on?.workflow_call;
  for (const name of ['version', 'release_commit']) {
    const input = trigger?.inputs?.[name];
    if (input?.type !== 'string' || input.required !== true) {
      failIntegrity(`docs workflow_call must require the ${name} string`);
    }
  }
  const stable = trigger?.inputs?.stable;
  if (stable?.type !== 'boolean' || stable.default !== false) {
    failIntegrity('docs workflow_call stable must be an optional false boolean');
  }
}

function checkDocsWorkflowSettings(docsDocument) {
  if (!docsDocument.on?.push?.branches?.includes('main')) {
    failIntegrity('docs workflow must publish mutable development docs from main');
  }
  if (!(docsDocument.on && 'workflow_dispatch' in docsDocument.on)) {
    failIntegrity('docs workflow must support manual development publication after Pages setup');
  }
  if (docsDocument.permissions?.contents !== 'write') {
    failIntegrity('docs workflow must receive contents write permission');
  }
  if (
    docsDocument.permissions?.pages !== 'write' ||
    docsDocument.permissions?.['id-token'] !== 'write'
  ) {
    failIntegrity('docs workflow must receive Pages and OIDC write permissions');
  }
  if (docsDocument.concurrency?.['cancel-in-progress'] !== false) {
    failIntegrity('docs workflow must serialize publishers without cancelling releases');
  }
}

function checkDocsDelegation(releaseDocument, docsDocument) {
  checkDocsReleaseJob(releaseDocument);
  checkDocsWorkflowInputs(docsDocument);
  checkDocsWorkflowSettings(docsDocument);
}

function checkOptionalPyPiInputs(releaseDocument, pythonDocument) {
  const releaseInput = releaseDocument.on?.workflow_dispatch?.inputs?.publish_pypi;
  if (releaseInput?.type !== 'boolean' || releaseInput.default !== true) {
    failIntegrity('release workflow must default publish_pypi to true');
  }
  const delegatedInput = releaseDocument.jobs?.['python-sdk-release']?.with?.publish_pypi;
  if (delegatedInput !== '${{ inputs.publish_pypi }}') {
    failIntegrity('release workflow must pass publish_pypi to the Python SDK workflow');
  }

  for (const trigger of ['workflow_dispatch', 'workflow_call']) {
    const input = pythonDocument.on?.[trigger]?.inputs?.publish_pypi;
    if (input?.type !== 'boolean' || input.default !== true) {
      failIntegrity(`release-python ${trigger} must default publish_pypi to true`);
    }
  }
}

function checkOptionalPyPiSteps(pythonDocument) {
  const publishSteps = new Map(
    pythonDocument.jobs?.publish?.steps?.map((step) => [step.name, step]) ?? []
  );
  const expectedConditions = new Map([
    ['Setup Python 3.12', 'inputs.publish_pypi'],
    ['Verify existing PyPI version before recovery', 'inputs.publish_pypi'],
    ['Record intentionally deferred PyPI publication', 'inputs.publish_pypi == false'],
    [
      'Publish Python SDK with trusted publishing',
      "inputs.publish_pypi && steps.pypi.outputs.exists == 'false'",
    ],
  ]);
  for (const [name, condition] of expectedConditions) {
    if (publishSteps.get(name)?.if !== condition) {
      failIntegrity(`release-python ${name} must use condition: ${condition}`);
    }
  }
  if (publishSteps.get('Create or complete independent SDK GitHub Release')?.if !== undefined) {
    failIntegrity('release-python GitHub wheel publication must not depend on publish_pypi');
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
    'publish_pypi: ${{ inputs.publish_pypi }}',
    'uses: ./.github/workflows/docs.yml',
    'version: ${{ needs.plan.outputs.tag }}',
    'release_commit: ${{ needs.plan.outputs.commit }}',
    "stable: ${{ needs.plan.outputs.publish-latest == 'true' }}",
  ]);
}

function checkDocsWorkflowFragments(workflow) {
  requireFragments('docs workflow', workflow, [
    'ZEROSHOT_DOCS_VERSION:',
    'ZEROSHOT_DOCS_COMMIT:',
    'development documentation must use current main $main_commit',
    'python -m pip install --disable-pip-version-check -r docs/requirements.lock',
    'cargo run --locked -p zeroshot --example generate_cli_docs -- --check',
    'cargo run --locked -p openengine-cluster-testkit --bin generate-cluster-protocol -- --check',
    'python -m mkdocs build --strict',
    'site/manifest.json',
    'id="zeroshot.Client"',
    'id="zeroshot.RunResult"',
    'id="zeroshot.InvalidRequestError"',
    'refs/remotes/origin/gh-pages:${DOCS_VERSION}/manifest.json',
    '$DOCS_VERSION is immutable and already belongs to $existing_commit',
    'cmp -s "$remote_manifest" site/manifest.json',
    'required_snapshot_paths=(',
    '$DOCS_VERSION is missing $relative',
    'required_python_anchors=(',
    '$DOCS_VERSION has an invalid rendered Python API at $relative',
    "steps.published.outputs['exact-exists'] == 'false'",
    'mike deploy',
    'mike alias',
    '--alias-type redirect',
    'mike set-default',
    'actions/configure-pages@983d7736d9b0ae728b81ab479565c72886d7745b',
    'actions/upload-pages-artifact@7b1f4a764d45c48632c6b24a0339c27f5614fb0b',
    'actions/deploy-pages@d6db90164ac5ed86f2b6aed7e0febac5b3c0c03e',
  ]);
  if (workflow.includes('python -m mike')) {
    failIntegrity('docs workflow must invoke the installed mike console script');
  }
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
    'if: inputs.publish_pypi == false',
    "if: inputs.publish_pypi && steps.pypi.outputs.exists == 'false'",
    'Rerun this workflow with the same immutable inputs and publish_pypi enabled to recover.',
  ]);
  if (workflow.includes('skip-existing: true')) {
    failIntegrity('release-python workflow must verify existing files instead of skipping them');
  }
  if (workflow.includes('continue-on-error')) {
    failIntegrity('release-python workflow must not hide publication failures');
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
  const pythonDocument = parsePythonReleaseWorkflow(contract.pythonWorkflow);
  const docsDocument = parseDocsWorkflow(contract.docsWorkflow);
  checkReleaseJobs(document);
  checkDocsDelegation(document, docsDocument);
  checkOptionalPyPiInputs(document, pythonDocument);
  checkOptionalPyPiSteps(pythonDocument);
  checkObsoleteWorkflowIdentities(contract.workflow);
  checkReleaseFragments(contract.workflow);
  checkPythonReleaseFragments(contract.pythonWorkflow);
  checkDocsWorkflowFragments(contract.docsWorkflow);
  checkTargetParity(contract.shimTargets);
  checkToolingManifest(contract.packageManifest, contract.packageLock);
  return true;
}

module.exports = { checkRepository };
