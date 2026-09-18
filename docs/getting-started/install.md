# Install Zeroshot

Install the `zeroshot` command and its agent skill through the npm delivery package, which requires
Node.js 18 or newer. The installer selects the release target declared for the current system and
checks the downloaded archive against `SHA256SUMS` before writing the native executable.

```console
npm install --global @the-open-engine-company/zeroshot
```

Release builds cover Linux x64 and arm64, macOS x64 and arm64, and Windows x64. Node.js is only an
installer dependency; the command itself is a Rust executable.

The package installs one managed skill for Codex and GitHub Copilot under `$HOME/.agents/skills`,
and for Claude Code under `${CLAUDE_CONFIG_DIR:-$HOME/.claude}/skills`. No agent selection or
per-project installation is needed. npm 7 and newer do not run uninstall hooks; after removing the
package, delete its two skill directories manually only if their `SKILL.md` files remain unmodified.

On Windows, the CLI and local in-process runs work natively from PowerShell or Command Prompt.
Install Git for Windows and put Git and your chosen harness on `PATH`; npm-installed `.cmd`
launchers are supported. Local configuration defaults to `%LOCALAPPDATA%\zeroshot` and run state
to `%LOCALAPPDATA%\zeroshot\state`. `ZEROSHOT_CONFIG_DIR` and `ZEROSHOT_STATE_DIR` can override
these with absolute paths. The hosted target image requires Linux.

For Codex, complete its [native Windows sandbox setup](https://developers.openai.com/codex/windows)
before running graphs with reviewers. Zeroshot keeps local Codex reviewers read-only; it does not
install their sandbox.

## Connect Zeroshot Cloud

The Cloud target is built in. Sign in once; organization selection happens in the browser:

```console
zeroshot target login cloud
```

Cloud runs use `--target cloud`. Omit `--target` for local execution.

## Open the workspace UI

```console
zeroshot ui
```

Open `http://127.0.0.1:4173/ui/` to edit profiles and inspect live or completed local runs.
Use `--listen 127.0.0.1:4185` to choose another loopback port. Ctrl-C stops the UI server;
active runs continue. Restart the command to reconnect.

**Profiles** edits graphs and runtime settings; the info icon explains authoring defaults.
Saved profiles share the CLI's configuration store (`ZEROSHOT_CONFIG_DIR`); run history uses
`ZEROSHOT_STATE_DIR`. Start a saved local profile from the CLI:

```console
zeroshot run --title "My task" --profile local:my-profile --input input.json
```

Open **Runs** for [live monitoring and replay](../guides/observe-and-control.md#browser).
The [Docker target](../concepts/targets.md#direct-target) serves its own profiles and runs at `/ui/`.
Cloud embedding remains tracked in [zero-cloud #301](https://github.com/the-open-engine/zero-cloud/issues/301).

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

Run Zeroshot from a Git worktree, and install the agent harness named by the runtime plan:

- install and sign in to Codex for `"harness": "codex"`;
- install and sign in to Claude Code for `"harness": "claude"`;
- install GitHub Copilot CLI 1.0.86 for `"harness": "copilot"` with `"provider": "github"`.

Provider credentials can come from the current environment or the private Zeroshot connection
store. The command below prompts without echo and keeps the value out of runtime JSON:

```console
zeroshot connection set openai --field OPENAI_API_KEY
```

Use `openrouter`, `anthropic`, `bedrock`, or `gateway` as the connection key when the runtime declares that
provider. [Runtimes and connections](../concepts/runtimes-and-connections.md) lists the default field
names and explains the separation.

## Other installation paths

Each [GitHub release](https://github.com/the-open-engine/zeroshot/releases) includes native archives
and `SHA256SUMS`; archive names contain the release version and target triple.

The Python SDK requires Python 3.11 or newer and ships the matching executable inside each platform
wheel:

```console
python -m pip install the-open-engine-zeroshot
```

For a long-running target, use `ghcr.io/the-open-engine/zeroshot-target`. See the
[targets guide](../concepts/targets.md) for a Docker setup bound to loopback.

[Validate and run a first task](first-run.md) after installation.

## GitHub Copilot

Copilot uses your GitHub user identity and Copilot entitlement. Store a user OAuth token or
fine-grained personal token with Copilot Requests permission in the `github` connection's
`COPILOT_GITHUB_TOKEN` field. A GitHub App installation token is not the user-backed route.
The token stays in private RPC and is excluded from agent tool environments.

```json
{ "harness": "copilot", "provider": "github", "model": "auto" }
```

Model IDs pass unchanged to Copilot. Zeroshot uses headless RPC schema output and validates each
response locally, allowing at most two correction turns in the same session. Install the pinned
CLI version above; the target image already includes it.

Hosted connection resolvers may additionally declare `COPILOT_GITHUB_TOKEN_EXPIRES_AT` (Unix
seconds). Copilot then requests credentials through the resolver throughout a long execution.
The resolver must return a refreshed token with more than one hour remaining. Static tokens
without expiry metadata are supplied once when each provider process starts.
