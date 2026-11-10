---
description: 'Installer verification, update behavior, managed skills and building the workspace UI.'
---

# Installation and updates

[Install zeroshot](../getting-started/install.md) covers setup. This page describes how installation and updates behave.

## Archive verification

Install the `zeroshot` command and its agent skills through the npm delivery package, which requires
Node.js 18 or newer. The installer selects the release target declared for the current system and
checks the downloaded archive against `SHA256SUMS` before writing the native executable.

```console
npm install --global @the-open-engine-company/zeroshot
```

## Update behavior

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

Older updaters install only the skills they know. If `zeroshot-graph-design` or `zeroshot-plan` is
missing after an executable upgrade, run `zeroshot update` again. The current updater refreshes all
managed skills even when the executable version is already current.

## Managed skills

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

## Windows paths

On Windows, the CLI and local in-process runs work natively from PowerShell or Command Prompt.
Install Git for Windows and put Git and your chosen harness on `PATH`; npm-installed `.cmd`
launchers are supported. Local configuration defaults to `%LOCALAPPDATA%\zeroshot` and run state
to `%LOCALAPPDATA%\zeroshot\state`. `ZEROSHOT_CONFIG_DIR` and `ZEROSHOT_STATE_DIR` can override
these with absolute paths. The hosted target image requires Linux.

## Build the workspace UI

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

## Release assets

Each [GitHub release](https://github.com/the-open-engine/zeroshot/releases) includes native archives,
the canonical `zeroshot-skill.md`, `zeroshot-memory-init-skill.md`,
`zeroshot-graph-design-skill.md`, and `zeroshot-plan-skill.md`, and `SHA256SUMS`; archive
names contain the release version and target triple.
