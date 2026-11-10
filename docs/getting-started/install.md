---
description: 'Install the zeroshot CLI and its agent skills, connect Cloud, open the workspace UI and prepare a local run.'
---

# Install Zeroshot

To use Zeroshot from ChatGPT, Codex, or Claude without installing this CLI,
[install the plugin and connect MCP](../guides/plugins.md).

Install the `zeroshot` command and its agent skills through the npm delivery package, which requires
Node.js 18 or newer. The installer selects the release target declared for the current system and
checks the downloaded archive against `SHA256SUMS` before writing the native executable.

```console
npm install --global @the-open-engine-company/zeroshot
```

A canonical CLI release can update its executable and managed agent skills in place. The command
selects the newest GitHub release for the current platform, verifies its archive and skill assets
against that release's `SHA256SUMS`, and verifies the new executable before replacing the old one:

```console
zeroshot update
```

The update does not elevate privileges; the current executable must be writable by the caller.
Updates of one installation replace it one at a time. An update that waited for another one and
finds the same release already installed reports `"updated":false`. Before installing a newer
release, an update also removes the files that interrupted updates left next to the executable.
A download fails with a timeout once the server has sent nothing for 30 seconds; a slow download
that keeps receiving data is never cut off. Failures name their cause, such as an unreachable or
refusing proxy, an untrusted certificate, or GitHub's explanation of a rate limit.
Unchanged managed skill copies are installed or refreshed for Codex, GitHub Copilot, Pi, and Claude
Code. A user-edited or conflicting skill is preserved, and the command fails with its exact path
after completing any safe updates.

Versions 10.4.0 through 10.8.0 cannot update themselves to a current release. Their updater accepts
only the `zeroshot` executable, but every archive since 10.9.0 also contains the `restic` sidecar,
so `zeroshot update` always fails with `release archive contains unexpected entry restic`
(`restic.exe` on Windows) and leaves the installation unchanged. Install the latest release through
npm once; later updates can use `zeroshot update` again:

```console
npm install --global @the-open-engine-company/zeroshot@latest
```

Older updaters install only the skills they know. If `zeroshot-graph-design` or `zeroshot-plan` is
missing after an executable upgrade, run `zeroshot update` again. The current updater refreshes all
managed skills even when the executable version is already current.

Release builds cover Linux x64 and arm64, macOS x64 and arm64, and Windows x64. Node.js is only an
installer dependency; the command itself is a Rust executable.

The package installs the `zeroshot`, `zeroshot-memory-init`, `zeroshot-graph-design`, and
[`zeroshot-plan`](../guides/planning.md) skills for Codex, GitHub Copilot, and Pi under
`$HOME/.agents/skills/<name>`, and for Claude Code under
`${CLAUDE_CONFIG_DIR:-$HOME/.claude}/skills/<name>`. No agent selection or
per-project installation is needed. npm 7 and newer do not run uninstall hooks; after removing the
package, remove only unmodified managed skill directories from these locations.
With automatic skill installation enabled, `CLAUDE_CONFIG_DIR` must be absolute when set so the
global skills have stable locations.
Use `zeroshot-graph-design` for a guided interview to design a graph for your repository.

To install or update only the executables, set `ZEROSHOT_SKIP_SKILLS=1`:

```console
ZEROSHOT_SKIP_SKILLS=1 npm install --global @the-open-engine-company/zeroshot
ZEROSHOT_SKIP_SKILLS=1 zeroshot update
```

Only the exact value `1` enables the opt-out. Both commands skip all user-scope skill installation
and leave existing skills untouched, even when destinations conflict or are unwritable.
Export the variable in your shell configuration to keep future updates from recreating skills you
remove. In PowerShell, set `$env:ZEROSHOT_SKIP_SKILLS = '1'` before running either command.
Unset the variable to restore automatic installation and refresh of unchanged managed skills.

See [Active Memory storage and backups](../guides/active-memory.md#persistence-and-run-completion)
for the local lesson database and how to preserve it when moving or reinstalling Zeroshot.

On Windows, the CLI and local in-process runs work natively from PowerShell or Command Prompt.
Install Git for Windows and put Git and your chosen harness on `PATH`; npm-installed `.cmd`
launchers are supported. Local configuration defaults to `%LOCALAPPDATA%\zeroshot` and run state
to `%LOCALAPPDATA%\zeroshot\state`. `ZEROSHOT_CONFIG_DIR` and `ZEROSHOT_STATE_DIR` can override
these with absolute paths. The hosted target image requires Linux.

For Codex, complete its [native Windows sandbox setup](https://developers.openai.com/codex/windows)
if your Codex permission settings require it. Workers and reviewers use the same permission handling;
Zeroshot does not install the sandbox.

## Connect Zeroshot Platform

The Platform target is built in. Sign in once; organization selection happens in the browser:

```console
zeroshot target login platform
```

Platform runs use `--target platform`. Omit `--target` for local execution. The
[Zeroshot Platform docs](https://app.zeroshot.sh/docs) cover organizations, the GitHub App, and saved
connections.

## Open the workspace UI

```console
zeroshot ui
```

Open `http://127.0.0.1:4173/ui/` to edit profiles and inspect live or completed local runs.
Use `--listen 127.0.0.1:4185` to choose another loopback port. Ctrl-C stops the UI server;
active runs continue. Restart the command to reconnect.

To inspect runs on a configured target while keeping profiles in the local CLI store:

```console
zeroshot ui --target docker
```

Use `--target platform` after signing in to inspect Platform runs through the same local UI.

**Profiles** edits graphs and runtime settings; the info icon explains authoring defaults.
Saved profiles share the CLI's configuration store (`ZEROSHOT_CONFIG_DIR`); without `--target`,
run history uses `ZEROSHOT_STATE_DIR`. Start a saved local profile from the CLI:

```console
zeroshot run --title "My task" --profile local:my-profile --input input.json
```

Open **Runs** for [live monitoring and replay](../guides/observe-and-control.md#browser).
The [Docker target](../concepts/targets.md#direct-target) serves its own profiles and runs at `/ui/`.
Embedding hosts use the [workspace bridge contract](https://github.com/the-open-engine/zeroshot/blob/main/ui/README.md#platform-integration).

Release executables embed the UI. To build it from source:

```console
npm --prefix ui ci --ignore-scripts
npm --prefix ui run build
cargo build --release --package zeroshot --features ui
./target/release/zeroshot ui
```

Node.js is needed for this build, not to serve the resulting UI. Library consumers
can leave the `ui` feature disabled.

## Prepare a local run

Choose an existing working folder with `--cwd FOLDER`, or use the invoking folder. Local runs without
Git delivery do not need a repository. Install a current version of each native harness CLI named by
the runtime plan and put it on `PATH`; Claude Code must be 2.1.310 or newer. Official local builds include contributed OpenCode and Hermes adapters; see
[Harness adapters](../project/harness-adapters.md) for availability and provider support.

Local GitHub pull request delivery (`--pr` or `--ship`) also needs the GitHub CLI, `gh`, on `PATH`.
Zeroshot runs the `gh` that `PATH` finds and rejects the run before any agent starts if there is
none.
See [forge setup](../guides/forge-delivery.md) for host logins and token connections.

Local lanes with native-login support reuse the installed harness's login and configuration. Other
provider lanes can read declared values from the current environment or the private Zeroshot
connection store. For example, this command prompts without echo and keeps an OpenAI value out of
runtime JSON for a contained target:

```console
zeroshot connection set openai --field OPENAI_API_KEY
```

Use `openrouter`, `anthropic`, `bedrock`, `foundry`, or `gateway` as the connection key when the effective
runtime declares that provider. [Runtimes and connections](../concepts/runtimes-and-connections.md)
lists the automatic requirements and explains the separation.

## Other installation paths

Each [GitHub release](https://github.com/the-open-engine/zeroshot/releases) includes native archives,
the canonical `zeroshot-skill.md`, `zeroshot-memory-init-skill.md`,
`zeroshot-graph-design-skill.md`, and `zeroshot-plan-skill.md`, and `SHA256SUMS`; archive
names contain the release version and target triple.

The Python SDK requires Python 3.11 or newer and ships the matching executable inside each platform
wheel. Download the wheel for your [OS and CPU](../guides/python-sdk.md#install-and-run) from [Python SDK releases](https://github.com/the-open-engine/zeroshot/releases?q=zeroshot-python-v).
Replace `WHEEL_FILE` with its local path:

```console
python -m pip install WHEEL_FILE
```

For a long-running target, use `ghcr.io/the-open-engine/zeroshot-target`. See the
[targets guide](../concepts/targets.md) for a Docker setup bound to loopback.

[Validate and run a first task](first-run.md) after installation.

## GitHub Copilot

Copilot's `github` lane uses your GitHub user identity and Copilot entitlement. Local runs on that
lane reuse the login stored by `copilot login` in `COPILOT_HOME` or the system credential store. Ambient
`COPILOT_GITHUB_TOKEN`, `GH_TOKEN`, or `GITHUB_TOKEN` values stay in private RPC and are excluded
from agent tool environments. For a contained target, store a user OAuth token or fine-grained
personal token with Copilot Requests permission in the `github` connection's
`COPILOT_GITHUB_TOKEN` field. A GitHub App installation token is not the user-backed route.

Local Copilot BYOK settings such as `COPILOT_PROVIDER_BASE_URL`, the static provider credential,
headers, wire API, and wire-model mapping are also reused. Zeroshot transfers that provider
configuration into the private headless RPC session and strips its credential fields and native
credential-store locators from tools. Command-backed keys configured with
`COPILOT_PROVIDER_API_KEY_COMMAND` cross that private provider contract, so the pinned CLI refreshes
them for each provider request. Eligible helper-only invoking-shell fields are exhaustively marked
through Copilot's `--secret-env-vars`, keeping them out of shell and MCP tools; runtime, auth,
provider, and parent-process loader controls are excluded. Zeroshot also reads a bounded
`COPILOT_PROVIDERS_CONFIG` or default `COPILOT_HOME/providers.json` registry and transfers its
providers and models into the headless session; a nonempty registry takes precedence over legacy
provider variables. An explicit declared GitHub token suppresses ambient BYOK and offline controls.
Declared credentials replace other ambient credential forms, and a declared endpoint does not
inherit ambient credentials or headers.

This is Copilot's native same-user boundary: secret fields are not inherited by ordinary shell or
MCP environments and are redacted from output, but the flag is not OS isolation from an adversarial
process running as your own user.

```json
{ "harness": "copilot", "provider": "github", "model": "auto" }
```

Model IDs pass unchanged to the Copilot process. When a registry entry with `apiKeyCommand` must use
Copilot's singular provider contract, its authored `modelId` becomes the RPC session model and its
authored `wireModel` still names the provider request model. Zeroshot uses headless RPC schema output
and validates each response locally, allowing at most two correction turns in the same session.
The target image already includes the native Copilot CLI.

Hosted connection resolvers may additionally declare `COPILOT_GITHUB_TOKEN_EXPIRES_AT` (Unix
seconds). Copilot then requests credentials through the resolver throughout a long execution.
The callback must match `COPILOT_GH_HOST` (or `GH_HOST`) when configured. The resolver must return a
refreshed token with more than one hour remaining. Static tokens
without expiry metadata are supplied once when each provider process starts.
