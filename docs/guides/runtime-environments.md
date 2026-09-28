# Prepare a runtime environment

Run submissions to Docker targets accept an `environment` field alongside `runtime`:

```json
{
  "environment": {
    "setup": "apt-get update && apt-get install -y jq",
    "startup": "npm ci",
    "variables": { "CI": "true" }
  }
}
```

Profiles contain graph and agent settings and have no environment reference. Supply the concrete
definition when submitting a run. With the CLI, put the contents of `environment` above into a JSON
file and pass `--environment FILE` alongside your profile. `--no-environment` sends an explicit empty
definition (`{}`), which bypasses defaults on hosts that provide them. Omitting both flags leaves
selection to the host. Direct Docker uses the base environment when none is supplied.
Merge-plan manifests also accept a top-level `environment`; the definition applies to each run
in that plan, while omission allows the host to resolve each repository's default.

Hosts may provide saved environments and choose repository defaults before accepting a run.
Zeroshot executes the resolved definition and preserves it for queued and resumed attempts; it
does not store environment resources or interpret repository defaults. The local UI has no
environment catalog or selector. Connection values come from the existing target connection
mechanism and never belong in scripts or public variables. Only explicitly declared hook
connections reach preparation; node connections remain independently declared.
If installation needs private registry access, add
`"connections": { "package-registry": ["NPM_TOKEN"] }` to the definition.
The CLI can collect hook credentials for a definition you supply. When a host selects a default,
its connection store must supply those credentials; the CLI does not inspect saved environments
to discover additional secrets in your shell.

## Example: pinned Node and npm dependencies

For a repository with a committed `package-lock.json`, save this as `setup.sh`. It installs OS
packages and Node 22.22.3, verifies the archive against the release's SHA-256 checksums, and exposes
Node to every agent through `$ZEROSHOT_TOOLS/bin`:

```bash
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y --no-install-recommends jq sqlite3 xz-utils

case "$(uname -m)" in
  x86_64) arch=x64 ;;
  aarch64) arch=arm64 ;;
  *) echo 'Unsupported Node architecture' >&2; exit 1 ;;
esac
version=22.22.3
release="node-v$version-linux-$arch"
download=$(mktemp -d)
trap 'rm -rf "$download"' EXIT
curl -fsSL "https://nodejs.org/dist/v$version/$release.tar.xz" -o "$download/$release.tar.xz"
curl -fsSL "https://nodejs.org/dist/v$version/SHASUMS256.txt" -o "$download/SHASUMS256.txt"
(cd "$download" && sha256sum --check --ignore-missing SHASUMS256.txt)
tar -xJf "$download/$release.tar.xz" -C "$ZEROSHOT_TOOLS"
for tool in node npm npx; do
  ln -sfn "../$release/bin/$tool" "$ZEROSHOT_TOOLS/bin/$tool"
done
node --version
```

Save the workspace preparation as `startup.sh`. `npm ci` rebuilds `node_modules` from the lockfile,
so this also works after restoring a workspace:

```bash
set -euo pipefail
test "$(node --version)" = v22.22.3
npm ci --no-audit --no-fund
```

On your machine, use `jq` to embed both scripts in a flat environment file, then submit it with
your existing profile, task input file, and target:

```bash
jq -n --rawfile setup setup.sh --rawfile startup startup.sh \
  '{setup: $setup, startup: $startup, variables: {CI: "true"}}' > environment.json
zeroshot run --profile my-profile --target my-target \
  --environment environment.json --title "Fix the failing test" --input input.json
```

The file contains `setup` and `startup` directly; an inline run API request puts the same object
in `submission.environment`, beside `submission.runtime`. Setup cannot read repository files because
checkout has not happened yet. Startup can invoke a checked-in script, for example
`bash .zeroshot/startup.sh`.

## Lifecycle and installation conventions

Submission returns after admission and durable acceptance. While the run is `admitted`, setup runs
as root before source checkout or workspace restoration. Startup then runs as the workspace user,
with the checkout as its current directory. The graph starts only after both succeed. Watch run
logs for phase progress and command output. Nonzero exits become `environment_setup_failed` or
`environment_startup_failed`; a preparation deadline becomes `environment_preparation_timeout`.
These stop the run without using any graph repair attempts. Force-stop works during preparation
and waits for process cleanup before recording its terminal result.

Each hook uses Bash with `errexit` and `pipefail` and has fifteen minutes, inside one thirty-minute
preparation budget. Install OS packages in setup. Install shared executable tools under
`$ZEROSHOT_TOOLS/bin`, which is on the PATH of every hook and agent, and project dependencies in
the workspace. Shell exports affect only that hook; use `variables` for later commands. The
platform's harness Node interpreter remains separate from a project-installed Node version. Setup,
startup, and agent sessions have separate home directories; use `$ZEROSHOT_TOOLS` for tools shared
across them.

Workers and reviewers share files, ignored dependencies, generated output, and services. Startup
can start a background service; its process belongs to the run and survives individual agent
executions. Wait for required services to be ready before startup exits; the runtime does not infer
their health. Setup is for installation: start services from startup. Do not daemonize processes or
start root services in setup. Setup cancellation cleans up its shell process group; a root script
can escape that group, so this is not containment for untrusted root code. The target images
suppress Debian package service autostart. Parallel graph branches share mutable state, so scripts
and graphs must coordinate concurrent writes.

Both hooks run again on resume: setup, restore workspace files, startup, then restart or continue
the graph. Startup must be idempotent with respect to existing workspace files. Checkpoints do not
restore running services, Docker images or volumes, database transactions, or agent conversations.
A background process may change files while a checkpoint is captured; use application-managed
exports for consistent database recovery.

A direct Docker target is an operator-trusted container. Root setup changes that container's OS,
including for other runs it hosts. Use separate disposable target containers for independent OS
requirements. The target must have a writable root for package installation. Docker commands need
an operator-provided daemon endpoint. Detached Docker containers belong to that daemon and are not
terminated by the target's process cleanup. Direct target operators or scripts must manage their
names, reuse, and removal. When the daemon runs inside a disposable run machine, destroying that
machine removes its Docker resources. Local execution uses the invoking machine and rejects setup
and startup hooks and their connection references.
