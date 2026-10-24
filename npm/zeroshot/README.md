# `@the-open-engine-company/zeroshot`

Zeroshot enforces your team's QA process on coding agents. You define the process as a graph:
which agents implement, which reviewers and tests a change must pass, where failures go for repair,
and where a run stops. Every task runs through it, and no agent can skip a step. It works with
Codex, Claude Code, GitHub Copilot and Pi.

This package is the installer for the canonical `zeroshot` executable and agent skills. The package selects the release
archive for the current Node platform and architecture, verifies it against that release's
`SHA256SUMS`, and installs the executable.

Read the [Zeroshot documentation](https://zeroshot.sh/docs/) for installation,
runtime configuration, targets, and CLI reference.

The product site is [zeroshot.sh](https://zeroshot.sh/?utm_source=npm&utm_medium=readme&utm_campaign=zeroshot).

The `zeroshot`, `zeroshot-memory-init`, and `zeroshot-graph-design` managed skills share one source
each across hosts.
They install for Codex, GitHub Copilot, and Pi at `$HOME/.agents/skills/<name>/SKILL.md`, and for
Claude Code at `${CLAUDE_CONFIG_DIR:-$HOME/.claude}/skills/<name>/SKILL.md`. Reinstalling updates an unchanged
managed copy. A conflicting or edited skill is preserved; installation fails with the exact path so
the conflict cannot pass unnoticed.

When set, `CLAUDE_CONFIG_DIR` must be absolute so the managed skills have stable locations.

npm 7 and newer do not run uninstall lifecycle scripts. After uninstalling the package, remove the
managed skill directories above manually if their `SKILL.md` files are still unmodified managed copies.

Installation fails closed with `UNSUPPORTED_ZEROSHOT_HOST` when the host has no declared release target. Source compilation and cross-target substitution are not supported.

Supported hosts are Linux x64/arm64, macOS x64/arm64, and Windows x64.
