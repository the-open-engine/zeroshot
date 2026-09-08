'use strict';

const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { describe, it } = require('node:test');
const yaml = require('js-yaml');

const root = path.resolve(__dirname, '..', '..');
const read = (relative) => fs.readFileSync(path.join(root, relative), 'utf8');

describe('v8 hard cutover contract', () => {
  it('keeps Node private and tooling-only at the repository root', () => {
    const manifest = JSON.parse(read('package.json'));
    assert.equal(manifest.name, '@the-open-engine-company/zeroshot-repository');
    assert.equal(manifest.private, true);
    assert.equal(manifest.main, undefined);
    assert.equal(manifest.bin, undefined);
    assert.equal(manifest.release, undefined);
    assert.equal(manifest.dependencies['js-yaml'] !== undefined, true);
  });

  it('has no legacy Node product directories or release workflow', () => {
    for (const relative of ['bin', 'cli', 'cluster-templates', 'lib', 'src', 'task-lib']) {
      assert.equal(fs.existsSync(path.join(root, relative)), false, relative);
    }
    assert.equal(fs.existsSync(path.join(root, '.github/workflows/release-rust.yml')), false);
    assert.equal(fs.existsSync(path.join(root, 'docker/zeroshot-rust-target')), false);
    assert.equal(fs.existsSync(path.join(root, 'npm/zeroshot-rust')), false);
    assert.equal(fs.existsSync(path.join(root, 'hooks')), false);
  });

  it('publishes one canonical product identity', () => {
    const release = read('.github/workflows/release.yml');
    assert.match(release, /^name: Release Zeroshot$/m);
    assert.match(release, /release_tag="v\$RELEASE_VERSION"/);
    assert.match(release, /ghcr\.io\/the-open-engine\/zeroshot-target/);
    assert.match(release, /existing_id.*candidate_id/);
    assert.match(release, /gh release edit "\$RELEASE_TAG" --latest="\$PUBLISH_LATEST"/);
    assert.match(release, /npm publish --provenance --access public \.\/shim-release\/\*\.tgz/);
    assert.doesNotMatch(release, /zeroshot-rust|release-rust|semantic-release/);
  });

  it('uses the requested Python distribution while preserving the import package', () => {
    const pyproject = read('sdks/python/pyproject.toml');
    const versionModule = read('sdks/python/src/zeroshot/_version.py');
    assert.match(pyproject, /^name = "the-open-engine-zeroshot"$/m);
    assert.match(pyproject, /setuptools==80\.9\.0/);
    assert.match(pyproject, /wheel==0\.45\.1/);
    assert.match(pyproject, /^zeroshot = \[/m);
    assert.match(versionModule, /version\("the-open-engine-zeroshot"\)/);
    assert.match(pyproject, /"PyYAML==6\.0\.3"/);
  });
});

describe('Python release publication contract', () => {
  it('requires an explicit opt-out to defer PyPI publication', () => {
    const release = yaml.load(read('.github/workflows/release.yml'));
    const pythonRelease = yaml.load(read('.github/workflows/release-python.yml'));
    const releaseInput = release.on.workflow_dispatch.inputs.publish_pypi;

    assert.equal(releaseInput.type, 'boolean');
    assert.equal(releaseInput.default, true);
    assert.equal(releaseInput.required, true);
    assert.equal(
      release.jobs['python-sdk-release'].with.publish_pypi,
      '${{ inputs.publish_pypi }}'
    );

    for (const trigger of ['workflow_dispatch', 'workflow_call']) {
      const input = pythonRelease.on[trigger].inputs.publish_pypi;
      assert.equal(input.type, 'boolean', trigger);
      assert.equal(input.default, true, trigger);
    }

    const publishSteps = new Map(pythonRelease.jobs.publish.steps.map((step) => [step.name, step]));
    assert.equal(publishSteps.get('Setup Python 3.12').if, 'inputs.publish_pypi');
    assert.equal(
      publishSteps.get('Verify existing PyPI version before recovery').if,
      'inputs.publish_pypi'
    );
    assert.equal(
      publishSteps.get('Record intentionally deferred PyPI publication').if,
      'inputs.publish_pypi == false'
    );
    assert.equal(
      publishSteps.get('Publish Python SDK with trusted publishing').if,
      "inputs.publish_pypi && steps.pypi.outputs.exists == 'false'"
    );
    assert.equal(
      publishSteps.get('Create or complete independent SDK GitHub Release').if,
      undefined
    );
  });

  it('keeps release recovery exact and fail-closed', () => {
    const pythonRelease = read('.github/workflows/release-python.yml');
    assert.match(pythonRelease, /pypi\.org\/pypi\/the-open-engine-zeroshot/);
    assert.match(
      pythonRelease,
      /SOURCE_DATE_EPOCH: \$\{\{ needs\.plan\.outputs\.source-date-epoch \}\}/
    );
    assert.match(pythonRelease, /git show -s --format=%ct/);
    assert.match(pythonRelease, /if remote != local:/);
    assert.match(pythonRelease, /GH_REPO: \$\{\{ github\.repository \}\}/);
    assert.match(pythonRelease, /gh release download "\$SDK_TAG"/);
    assert.match(pythonRelease, /"\$local_sha" == "\$remote_sha"/);
    assert.match(pythonRelease, /if: inputs\.publish_pypi == false/);
    assert.match(
      pythonRelease,
      /if: inputs\.publish_pypi && steps\.pypi\.outputs\.exists == 'false'/
    );
    assert.match(pythonRelease, /same immutable inputs and publish_pypi enabled to recover/);
    assert.doesNotMatch(pythonRelease, /skip-existing:\s*true/);
    assert.doesNotMatch(pythonRelease, /continue-on-error/);
  });
});

describe('v8 hard cutover contract', () => {
  it('keeps every workflow syntactically valid YAML', () => {
    const workflows = fs.readdirSync(path.join(root, '.github', 'workflows'));
    for (const filename of workflows.filter((name) => name.endsWith('.yml'))) {
      assert.doesNotThrow(() => yaml.load(read(path.join('.github', 'workflows', filename))));
    }
  });

  it('keeps the aggregate CI result fail-closed', () => {
    const workflow = read('.github/workflows/ci.yml');
    assert.match(workflow, /CLASSIFY_RESULT: \$\{\{ needs\.classify\.result \}\}/);
    assert.match(workflow, /\[\[ "\$CLASSIFY_RESULT" == "success" \]\]/);
    assert.match(workflow, /invalid selection value/);
  });

  it('announces the breaking v8 interface at the top level', () => {
    const readme = read('README.md');
    assert.match(readme, /Zeroshot v8 is a hard interface cutover/);
  });

  it('keeps repository-owned agent guidance aligned with v8', () => {
    const agents = read('AGENTS.md');
    const claude = read('CLAUDE.md');

    for (const guidance of [agents, claude]) {
      assert.match(guidance, /@the-open-engine-company\/zeroshot/);
      assert.match(guidance, /ghcr\.io\/the-open-engine\/zeroshot-target/);
      assert.match(guidance, /the-open-engine-zeroshot/);
      assert.match(guidance, /main/);
    }
    assert.doesNotMatch(claude, /npm i -g @the-open-engine\/zeroshot/);
    assert.doesNotMatch(claude, /cluster-templates\/base-templates/);
    assert.doesNotMatch(claude, /Feature branches merge into `dev`/);
    assert.doesNotMatch(claude, /semantic-release publishes/);
  });
});
