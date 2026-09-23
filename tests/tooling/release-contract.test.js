'use strict';

const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { describe, it } = require('node:test');
const yaml = require('js-yaml');

const root = path.resolve(__dirname, '..', '..');
const read = (relative) => fs.readFileSync(path.join(root, relative), 'utf8');
const assertPagesPublisherPermissions = (permissions) => {
  assert.deepEqual(permissions, {
    contents: 'write',
    'id-token': 'write',
    pages: 'write',
  });
};

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
    assert.match(
      release,
      /gh release edit "\$RELEASE_TAG" \\\n\s+--latest="\$PUBLISH_LATEST" \\\n\s+--notes-file "\$notes_file"/
    );
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

describe('Release-note publication contract', () => {
  it('generates one immutable notes artifact and republishes it exactly', () => {
    const workflow = yaml.load(read('.github/workflows/release.yml'));
    const planSteps = new Map(workflow.jobs.plan.steps.map((step) => [step.name, step]));
    const publishSteps = new Map(workflow.jobs.publish.steps.map((step) => [step.name, step]));
    const source = planSteps.get('Checkout exact Zeroshot release source');
    const generate = planSteps.get('Generate or recover immutable release notes');
    const upload = planSteps.get('Upload deterministic release notes');
    const download = publishSteps.get('Download deterministic release notes');
    const publish = publishSteps.get('Create or verify independent GitHub Release');

    assert.deepEqual(source.with, {
      'fetch-depth': 0,
      path: 'release-source',
      'persist-credentials': false,
      ref: '${{ inputs.release_commit }}',
    });
    assert.equal(
      planSteps.get('Verify exact main commit and independent tag')['working-directory'],
      'release-source'
    );
    assert.match(generate.run, /node release-source\/scripts\/release-notes\.js/);
    assert.match(generate.run, /--repo release-source/);
    assert.match(generate.run, /--commit "\$RELEASE_COMMIT"/);
    assert.match(generate.run, /gh release download "\$RELEASE_TAG"/);
    assert.match(generate.run, /cmp -- "\$notes_file" "\$generated_file"/);
    assert.equal(upload.with.name, 'zeroshot-release-notes-v${{ steps.release.outputs.version }}');
    assert.equal(
      upload.with.path,
      '${{ runner.temp }}/zeroshot-release-notes-v${{ steps.release.outputs.version }}.md'
    );
    assert.equal(upload.with['if-no-files-found'], 'error');
    assert.equal(download.with.name, 'zeroshot-release-notes-v${{ needs.plan.outputs.version }}');
    assert.equal(download.with.path, 'release-assets');
    assert.match(publish.run, /--notes-file "\$notes_file"/);
    assert.match(publish.run, /jq --join-output '\.body'/);
    assert.match(publish.run, /cmp -- "\$notes_file" "\$published_notes"/);
    assert.doesNotMatch(publish.run, /--notes "/);
    assert.doesNotMatch(publish.run, /expected_notes/);
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

describe('Embedded UI distribution', () => {
  it('builds locked UI assets before every released executable', () => {
    const release = yaml.load(read('.github/workflows/release.yml'));
    const commands = release.jobs.binaries.steps.map((step) => step.run || '').join('\n');
    const install = commands.indexOf('npm --prefix ui ci --ignore-scripts');
    const assets = commands.indexOf('npm --prefix ui run build');
    const binary = commands.indexOf('cargo build --release --locked');
    assert.ok(install >= 0 && assets > install && binary > assets);
    assert.match(commands, /cargo build[^\n]+--features ui[^\n]+--target/);
  });

  it('links the Windows release executable reproducibly before packaging it', () => {
    const release = yaml.load(read('.github/workflows/release.yml'));
    const steps = release.jobs.binaries.steps;
    const configureIndex = steps.findIndex(
      (step) => step.name === 'Configure reproducible MSVC linking'
    );
    const buildIndex = steps.findIndex(
      (step) => step.name === 'Build standalone Zeroshot release binary'
    );
    const configure = steps[configureIndex];

    assert.ok(configureIndex >= 0 && buildIndex > configureIndex);
    assert.equal(configure.if, "runner.os == 'Windows'");
    assert.equal(configure.shell, 'bash');
    assert.match(configure.run, /RUSTFLAGS=-C link-arg=\/Brepro/);
  });

  it('builds target UI assets independently of local generated files', () => {
    const dockerfile = read('docker/zeroshot-target/Dockerfile');
    const ignore = read('.dockerignore');
    assert.match(dockerfile, /FROM node:24-trixie-slim AS ui-builder/);
    assert.match(dockerfile, /COPY ui\/package\.json ui\/package-lock\.json/);
    assert.match(dockerfile, /npm --prefix ui ci --ignore-scripts/);
    assert.match(dockerfile, /npm --prefix ui run build/);
    assert.match(dockerfile, /COPY --from=ui-builder \/src\/ui\/dist \.\/ui\/dist/);
    assert.match(dockerfile, /cargo build[^\n]+--features ui/);
    assert.match(ignore, /^ui\/dist$/m);
    assert.match(ignore, /^\*\*\/node_modules$/m);
  });

  it('checks library and UI feature lanes plus native and Docker lifecycle smoke', () => {
    const workflow = yaml.load(read('.github/workflows/ci.yml'));
    const commands = workflow.jobs['native-check'].steps.map((step) => step.run || '').join('\n');
    assert.match(commands, /npm --prefix ui test/);
    assert.match(commands, /cargo clippy --workspace --all-targets -- -D warnings/);
    assert.match(commands, /cargo clippy --package zeroshot --all-targets --features ui/);
    assert.match(commands, /scripts\/distribution\/build-restic\.sh/);
    assert.match(commands, /ZEROSHOT_RESTIC_TEST=%s/);
    assert.match(commands, /ZEROSHOT_RESTIC="\$ZEROSHOT_RESTIC_TEST"/);
    assert.match(commands, /cargo test --workspace/);
    assert.match(commands, /cargo test --package zeroshot --features ui/);
    assert.match(commands, /smoke-ui\.js --binary target\/release\/zeroshot/);
    const imageCommands = workflow.jobs['target-image'].steps
      .map((step) => step.run || '')
      .join('\n');
    assert.match(imageCommands, /smoke-ui\.js --url http:\/\/127\.0\.0\.1:4185/);
    assert.match(imageCommands, /docker start "\$container"/);
    assert.match(imageCommands, /State\.ExitCode/);
  });
});

describe('CI efficiency contract', () => {
  it('cancels only superseded pull-request runs and classifies both sides of renames', () => {
    const source = read('.github/workflows/ci.yml');
    const workflow = yaml.load(source);

    assert.deepEqual(workflow.concurrency, {
      group: '${{ github.workflow }}-${{ github.event.pull_request.number || github.run_id }}',
      'cancel-in-progress': "${{ github.event_name == 'pull_request' }}",
    });
    assert.match(source, /git diff --no-renames --name-only -z/);
    assert.deepEqual(Object.keys(workflow.jobs.classify.outputs), [
      'native',
      'python',
      'tooling',
      'npm',
      'docs',
    ]);
  });

  it('pins the Rust cache without weakening native checks', () => {
    const workflow = yaml.load(read('.github/workflows/ci.yml'));
    const native = workflow.jobs['native-check'];
    const go = native.steps.find((step) => step.uses?.startsWith('actions/setup-go@'));
    const cache = native.steps.find((step) => step.uses?.startsWith('Swatinem/rust-cache@'));
    const rustdocs = native.steps.find(
      (step) => step.name === 'Build documentation with warnings denied'
    );
    const commands = native.steps.map((step) => step.run || '').join('\n');

    assert.equal(go.uses, 'actions/setup-go@b7ad1dad31e06c5925ef5d2fc7ad053ef454303e');
    assert.deepEqual(go.with, { 'go-version': '1.27.1', cache: false });
    assert.equal(cache.uses, 'Swatinem/rust-cache@6323deb102c322ba6fcbdcafc7e3dddab59af2b6');
    assert.deepEqual(cache.with, { 'cache-on-failure': true });
    assert.match(commands, /cargo clippy --workspace --all-targets -- -D warnings/);
    assert.match(commands, /cargo test --workspace/);
    assert.equal(rustdocs.env.RUSTDOCFLAGS, '-Dwarnings');
    assert.match(commands, /cargo build --locked --release/);
  });

  it('keeps npm and docs checks strict, cross-platform where relevant, and fail-closed', () => {
    const workflow = yaml.load(read('.github/workflows/ci.yml'));
    const npm = workflow.jobs['npm-check'];
    const docs = workflow.jobs['docs-check'];
    const required = workflow.jobs.required;
    const npmCommands = npm.steps.map((step) => step.run || '').join('\n');
    const docsCommands = docs.steps.map((step) => step.run || '').join('\n');

    assert.deepEqual(npm.strategy.matrix.os, ['ubuntu-latest', 'windows-latest']);
    const lineEndings = npm.steps.find(
      (step) => step.name === 'Preserve package line endings on Windows'
    );
    assert.equal(lineEndings.if, "runner.os == 'Windows'");
    assert.equal(lineEndings.run, 'git config --global core.autocrlf false');
    assert.match(npmCommands, /npm-package-install\.test\.js/);
    assert.match(npmCommands, /npm-skill-install\.test\.js/);
    assert.match(npmCommands, /npm run distribution:check/);
    assert.match(docsCommands, /docs-versions\.test\.js/);
    assert.match(docsCommands, /release-contract\.test\.js/);
    assert.match(docsCommands, /python -m mkdocs build --strict/);
    assert.match(docsCommands, /id="zeroshot\.Client"/);
    assert.match(docsCommands, /id="zeroshot\.RunResult"/);
    assert.match(docsCommands, /id="zeroshot\.InvalidRequestError"/);
    assert.equal(required.needs.includes('npm-check'), true);
    assert.equal(required.needs.includes('docs-check'), true);
    assert.equal(required.steps[0].env.NPM_SELECTED, '${{ needs.classify.outputs.npm }}');
    assert.equal(required.steps[0].env.DOCS_RESULT, '${{ needs.docs-check.result }}');
    assert.match(required.steps[0].run, /require_result "\$NPM_SELECTED" "\$NPM_RESULT"/);
    assert.match(required.steps[0].run, /require_result "\$DOCS_SELECTED" "\$DOCS_RESULT"/);
  });
});

describe('Native coverage contract', () => {
  it('measures every native test configuration and enforces useful coverage', () => {
    const workflow = yaml.load(read('.github/workflows/coverage.yml'));
    const coverage = workflow.jobs.coverage;
    const commands = coverage.steps.map((step) => step.run || '').join('\n');
    const install = coverage.steps.find((step) => step.name === 'Install coverage tooling');
    const publish = coverage.steps.find((step) => step.name === 'Publish coverage');

    assert.deepEqual(workflow.on.push.branches, ['main']);
    assert.deepEqual(workflow.on.pull_request.branches, ['main']);
    assert.equal(Object.hasOwn(workflow.on, 'workflow_dispatch'), true);
    assert.deepEqual(workflow.permissions, { contents: 'read' });
    assert.match(commands, /npm --prefix ui run build/);
    assert.match(commands, /cargo llvm-cov clean --workspace/);
    assert.match(commands, /cargo llvm-cov --workspace --no-report/);
    assert.match(
      commands,
      /cargo llvm-cov --package zeroshot --features ui --no-clean --summary-only/
    );
    assert.match(
      commands,
      /cargo llvm-cov --package zeroshot --bin zeroshot --features ui --no-clean --summary-only/
    );
    assert.match(commands, /cargo llvm-cov report --json --output-path coverage\.json/);
    assert.match(commands, /cargo llvm-cov report --lcov --output-path lcov\.info/);
    assert.match(commands, /const floors = \{ lines: 95, regions: 92, functions: 92 \}/);
    assert.match(commands, /regions\.count < 200/);
    assert.match(commands, /lines\.percent >= 75 && regions\.percent >= 75/);
    assert.equal(install.uses, 'taiki-e/install-action@7623a79cdfecb99d681017af368ca353d9f49bb5');
    assert.deepEqual(install.with, {
      tool: 'cargo-llvm-cov@0.9.0',
      fallback: 'none',
    });
    assert.equal(
      publish.uses,
      'coverallsapp/github-action@8d6379e14d29928660c4ba802d8e85393440b329'
    );
    assert.deepEqual(publish.with, {
      'github-token': '${{ secrets.GITHUB_TOKEN }}',
      file: 'lcov.info',
      format: 'lcov',
      'fail-on-error': true,
      'coverage-reporter-version': 'v0.6.22',
    });
  });
});

describe('Versioned documentation publication contract', () => {
  it('publishes Current from main and accepts exact releases for minor documentation', () => {
    const docs = yaml.load(read('.github/workflows/docs.yml'));

    assert.equal(docs.name, 'Publish versioned documentation');
    assert.deepEqual(docs.on.push.branches, ['main']);
    assert.equal(Object.hasOwn(docs.on, 'workflow_dispatch'), true);
    assert.equal(docs.on.workflow_call.inputs.version.type, 'string');
    assert.equal(docs.on.workflow_call.inputs.version.required, true);
    assert.equal(docs.on.workflow_call.inputs.release_commit.type, 'string');
    assert.equal(docs.on.workflow_call.inputs.release_commit.required, true);
    assert.equal(docs.on.workflow_call.inputs.stable.type, 'boolean');
    assert.equal(docs.on.workflow_call.inputs.stable.default, false);
    assertPagesPublisherPermissions(docs.permissions);
    assert.equal(docs.concurrency['cancel-in-progress'], false);
    assert.equal(docs.concurrency.queue, 'max');
    const rustSetup = docs.jobs.publish.steps.find((step) => step.name === 'Setup Rust 1.97.0');
    assert.deepEqual(rustSetup.with, {
      toolchain: '1.97.0',
      components: 'clippy,rustfmt',
    });
  });

  it('runs from the release chain after Python revision 1', () => {
    const release = yaml.load(read('.github/workflows/release.yml'));
    const docs = release.jobs.docs;

    assert.deepEqual(docs.needs, ['plan', 'python-sdk-release']);
    assertPagesPublisherPermissions(docs.permissions);
    assert.equal(docs.uses, './.github/workflows/docs.yml');
    assert.deepEqual(docs.with, {
      version: '${{ needs.plan.outputs.tag }}',
      release_commit: '${{ needs.plan.outputs.commit }}',
      stable: "${{ needs.plan.outputs.publish-latest == 'true' }}",
    });
  });

  it('checks generated references and guards publication identity', () => {
    const docs = read('.github/workflows/docs.yml');

    assert.match(docs, /generate_cli_docs -- --check/);
    assert.match(docs, /generate-cluster-protocol -- --check/);
    assert.match(docs, /pip install.*docs\/requirements\.lock/);
    assert.match(docs, /python -m mkdocs build --strict/);
    assert.match(docs, /Current documentation must use current main \$main_commit/);
    assert.match(docs, /id="zeroshot\.Client"/);
    assert.match(docs, /id="zeroshot\.RunResult"/);
    assert.match(docs, /id="zeroshot\.InvalidRequestError"/);
    assert.match(docs, /docs_versions\.py migrate/);
    assert.match(docs, /docs_versions\.py check-update/);
    assert.match(docs, /ZEROSHOT_PRODUCT_DOCS_VERSION/);
    assert.match(docs, /ZEROSHOT_DOCS_PUBLISHER_COMMIT/);
    assert.match(docs, /--alias-type redirect/);
    assert.match(docs, /mike set-default --config-file mkdocs.yml current/);
    assert.match(docs, /actions\/upload-pages-artifact@[0-9a-f]{40}/);
    assert.match(docs, /actions\/deploy-pages@[0-9a-f]{40}/);
    assert.doesNotMatch(docs, /python -m mike/);
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

  it('keeps the target data volume independent of internal protocol names', () => {
    const dockerfile = read('docker/zeroshot-target/Dockerfile');
    const targetReadme = read('docker/zeroshot-target/README.md');
    const readme = read('README.md');

    assert.match(dockerfile, /VOLUME \["\/var\/lib\/zeroshot"\]/);
    assert.doesNotMatch(dockerfile, /VOLUME \["\/var\/lib\/zeroshot\//);
    assert.match(targetReadme, /zeroshot-data:\/var\/lib\/zeroshot(?:\s|\\)/);
    assert.match(readme, /zeroshot-data:\/var\/lib\/zeroshot(?:\s|\\)/);
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
