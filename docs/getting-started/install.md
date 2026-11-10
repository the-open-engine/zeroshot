---
description: 'Install zeroshot, sign in to an agent CLI and run your first task. Includes updates, Windows setup and the workspace UI.'
---

# Install zeroshot

Install with Node.js 18 or newer:

```console
npm install --global @the-open-engine-company/zeroshot
```

This installs the native CLI and its agent skills. Releases support Linux and macOS on x64 and
arm64, and Windows on x64. Node.js is needed for installation; zeroshot itself is a Rust executable.

To use zeroshot through ChatGPT, Codex or Claude without the CLI,
[install the plugin](../guides/plugins.md).

## Prepare a local run

Install and sign in to the agent CLI you want to use: Codex, Claude Code, GitHub Copilot or Pi.
Put it on `PATH`. Claude Code requires version 2.1.310 or newer; see
[harness adapters](../project/harness-adapters.md) for all supported agents and providers.

Local runs can reuse supported native logins. Other providers and contained targets need
[connections](../concepts/runtimes-and-connections.md). For example:

```console
zeroshot connection set openai --field OPENAI_API_KEY
```

Choose a project folder. A local run with `--delivery none` edits that folder and needs no Git
repository. Opening a PR also requires a clean checkout, a remote and
[forge access](../guides/forge-delivery.md); GitHub delivery needs `gh` on `PATH`.

[Run a first task](first-run.md) walks through the input, agent configuration and run command.

### Windows

Use PowerShell or Command Prompt. Install Git for Windows and put Git and your agent CLI on
`PATH`; npm `.cmd` launchers work. If your Codex permissions require its
[native Windows sandbox](https://developers.openai.com/codex/windows), complete that setup too.
zeroshot does not install it. See [Windows paths](../reference/installation.md#windows-paths)
for configuration and state locations.

## Open the workspace UI

```console
zeroshot ui
```

Open `http://127.0.0.1:4173/ui/`. **Profiles** edits graphs and runtime settings; **Runs** shows
live runs and replays completed ones. Ctrl-C stops the UI server while active runs continue.
Use `--listen 127.0.0.1:4185` to choose another port.

To inspect runs on a configured target while keeping profiles local:

```console
zeroshot ui --target docker
```

Replace `docker` with your target's name, or `platform` after signing in. Profiles use
`ZEROSHOT_CONFIG_DIR`; local run history uses `ZEROSHOT_STATE_DIR`. To start a saved local profile:

```console
zeroshot run --title "My task" --profile local:my-profile --input input.json
```

See [monitoring and replay](../guides/observe-and-control.md#browser) or
[building the UI from source](../reference/installation.md#build-the-workspace-ui).

## Connect zeroshot Platform

```console
zeroshot target login platform
```

Choose your organization in the browser, then add `--target platform` to run commands.
The [Platform docs](https://app.zeroshot.sh/docs) cover accounts and saved connections.
For self-hosting, see [Targets](../concepts/targets.md#direct-target).

## Update

```console
zeroshot update
```

This updates the executable and unchanged managed skills. It preserves edited skills and reports
conflicts by path. If skills are missing after upgrading through an older updater, run the command
again. See [update behavior](../reference/installation.md#update-behavior) for verification and failures.

Versions 10.4.0 through 10.8.0 cannot update themselves: their updater rejects the bundled sidecar
with `release archive contains unexpected entry restic` (`restic.exe` on Windows).
Reinstall once through npm; subsequent updates can use `zeroshot update`:

```console
npm install --global @the-open-engine-company/zeroshot@latest
```

Updates preserve [Active Memory storage](../guides/active-memory.md#persistence-and-run-completion).

## Agent skills

The installer adds `zeroshot`, `zeroshot-plan`, `zeroshot-graph-design` and `zeroshot-memory-init`
for Codex, Claude Code, GitHub Copilot and Pi. No per-project setup is needed.

To install or update only the executables:

```console
ZEROSHOT_SKIP_SKILLS=1 npm install --global @the-open-engine-company/zeroshot
ZEROSHOT_SKIP_SKILLS=1 zeroshot update
```

In PowerShell, set `$env:ZEROSHOT_SKIP_SKILLS = '1'` before either command. Existing skills remain
untouched. Keep the variable in your shell configuration to preserve this choice across updates.
See [managed skills](../reference/installation.md#managed-skills) for paths and removal.

## GitHub Copilot

For local GitHub-backed runs, sign in with `copilot login`. Contained runs need a user token with
Copilot Requests permission; a GitHub App installation token cannot replace it.
[Copilot authentication](../reference/target-runtime.md#github-copilot) covers BYOK and token refresh.

## Other installation paths

- [Native archives](https://github.com/the-open-engine/zeroshot/releases) include checksums.
- The [Python SDK](../guides/python-sdk.md#install-and-run) bundles the matching executable and needs Python 3.11 or newer.
- The [Docker target](../concepts/targets.md#direct-target) runs zeroshot as a persistent service.
