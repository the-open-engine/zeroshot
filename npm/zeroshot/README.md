# `@the-open-engine-company/zeroshot`

Installer for the canonical `zeroshot` executable and agent skill. The package selects the release
archive for the current Node platform and architecture, verifies it against that release's
`SHA256SUMS`, and installs the executable.

Read the [Zeroshot documentation](https://the-open-engine.github.io/zeroshot/) for installation,
runtime configuration, targets, and CLI reference.

The same managed skill is installed for Codex and GitHub Copilot at
`$HOME/.agents/skills/zeroshot/SKILL.md`, and for Claude Code at
`${CLAUDE_CONFIG_DIR:-$HOME/.claude}/skills/zeroshot/SKILL.md`. Reinstalling updates an unchanged
managed copy. A conflicting or edited skill is preserved; installation fails with the exact path so
the conflict cannot pass unnoticed.

When set, `CLAUDE_CONFIG_DIR` must be absolute so the global skill has one stable location.

npm 7 and newer do not run uninstall lifecycle scripts. After uninstalling the package, remove the
two skill directories above manually if their `SKILL.md` files are still unmodified managed copies.

Installation fails closed with `UNSUPPORTED_ZEROSHOT_HOST` when the host has no declared release target. Source compilation and cross-target substitution are not supported.

Supported hosts are Linux x64/arm64, macOS x64/arm64, and Windows x64.
