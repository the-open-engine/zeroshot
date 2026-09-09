# Install Zeroshot

Install the `zeroshot` command through its npm delivery package, which requires Node.js 18 or newer.
The installer selects the release target declared for the current system and checks the downloaded
archive against `SHA256SUMS` before writing the native executable.

```console
npm install --global @the-open-engine-company/zeroshot
zeroshot version
```

Release builds cover Linux x64 and arm64, macOS x64 and arm64, and Windows x64. Node.js is only an
installer dependency; the command itself is a Rust executable.

## Prepare a local run

Run Zeroshot from a Git worktree, and install the agent harness named by the runtime plan:

- install and sign in to Codex for `"harness": "codex"`;
- install and sign in to Claude Code for `"harness": "claude"`.

Provider credentials can come from the current environment or the private Zeroshot connection
store. The command below prompts without echo and keeps the value out of runtime JSON:

```console
zeroshot connection set openai --field OPENAI_API_KEY
```

Use `openrouter`, `anthropic`, or `bedrock` as the connection key when the runtime declares that
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
