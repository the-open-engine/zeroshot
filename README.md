<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/brand/zeroshot-hero-dark.png">
  <img alt="Zeroshot. Self-driving software engineering. Layer 01 · Verification, The Open Engine." src="docs/brand/zeroshot-hero-light.png" width="100%">
</picture>

&nbsp;

<a href="https://theopenengine.com"><picture><source media="(prefers-color-scheme: dark)" srcset="docs/brand/social/website-dark.png"><img alt="Website" src="docs/brand/social/website-light.png" height="30"></picture></a>
<a href="https://x.com/OpenEngineHQ"><picture><source media="(prefers-color-scheme: dark)" srcset="docs/brand/social/x-dark.png"><img alt="X · @OpenEngineHQ" src="docs/brand/social/x-light.png" height="30"></picture></a>
<a href="https://www.linkedin.com/company/the-open-engine-company"><picture><source media="(prefers-color-scheme: dark)" srcset="docs/brand/social/linkedin-dark.png"><img alt="LinkedIn" src="docs/brand/social/linkedin-light.png" height="30"></picture></a>
<a href="https://discord.gg/9Tnxd7XWa"><picture><source media="(prefers-color-scheme: dark)" srcset="docs/brand/social/discord-dark.png"><img alt="Discord" src="docs/brand/social/discord-light.png" height="30"></picture></a>

[![npm](https://img.shields.io/npm/v/%40the-open-engine-company%2Fzeroshot?style=flat&labelColor=171411&color=171411)](https://www.npmjs.com/package/@the-open-engine-company/zeroshot)
[![CI](https://img.shields.io/github/actions/workflow/status/the-open-engine/zeroshot/ci.yml?style=flat&labelColor=171411&label=CI)](https://github.com/the-open-engine/zeroshot/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-171411?style=flat)](LICENSE)

</div>

# Zeroshot

**The agent that writes the code should not be the one that decides it works.**

Zeroshot turns a software goal into an explicit multi-agent graph. One agent implements. Independent
agents review. Failures route back into bounded repair. Delivery happens only after the graph's checks
pass.

> **Zeroshot v8 is a hard interface cutover.** The former Node.js runtime is retired; the native
> `zeroshot` executable is the product.

## Install

```bash
npm install -g @the-open-engine-company/zeroshot
zeroshot version
```

The npm package installs a verified native binary for Linux x64/arm64, macOS x64/arm64, or Windows
x64. Native archives and checksums are attached to each canonical `vX.Y.Z` GitHub Release.

## The graph is the orchestration

Agent loops hidden inside prompts are difficult to inspect, reproduce, or trust. Zeroshot makes the
control flow authored data: sequence, parallel review, retry paths, delivery, and exit conditions are
all explicit before a run starts.

The built-in `software-change` graph:

1. gives the goal to a worker;
2. runs acceptance and code review independently and in parallel;
3. routes rejected evidence to a repair worker and repeats both reviews;
4. with delivery enabled, delivers an accepted change through Git, CI, and merge;
5. routes delivery conflicts back through repair and review.

No runtime agent chooses the next step. **The graph is the orchestration.** Every transition is
bounded, and every event is written to a durable SQLite ledger.

<div align="center">
  <img src="docs/assets/zeroshot-demo.gif" alt="Animated Zeroshot software-change graph: a goal moves through implementation, parallel acceptance and code review, bounded repair loops, Git delivery, and a merged result" width="960">
  <br>
  <em>One authored graph: implement, review, repair when evidence fails, and deliver when it passes.</em>
</div>

Inspect the built-ins or bring your own graph:

```bash
zeroshot template list
zeroshot template show software-change
```

## One CLI, three environments

The graph and runtime plan stay the same. Only the target changes.

### Local: use your subscriptions

Run directly in your Git workspace. Local mode reuses an existing Codex or Claude Code login,
including subscription-backed sessions; explicit provider credentials can be bound instead.

```bash
zeroshot run \
  --title "Add JSON output with tests" \
  --template software-change \
  --ship \
  --input ./input.json \
  --runtime-config ./runtime.json
```

### Self-hosted: run the Docker target

Keep execution and durable state on infrastructure you control. The target image includes the native
engine plus pinned Codex and Claude harness CLIs.

```bash
docker run --rm --detach --name zeroshot-target \
  -p 127.0.0.1:8080:8080 \
  -v zeroshot-data:/var/lib/zeroshot \
  ghcr.io/the-open-engine/zeroshot-target:latest

zeroshot target add local --url http://127.0.0.1:8080 --direct
```

See the [target image guide](docker/zeroshot-target/README.md) for persistent storage, network
isolation, builds, and HTTPS.

### Zeroshot Cloud: close the laptop

Use the built-in `cloud` target at `https://api.cloud.zeroshot.sh` for a shared team queue and
durable run history:

```bash
zeroshot target login cloud
```

Open the printed link to sign in with the device code already filled in. Use `--target cloud` when
submitting runs.

## Reference

- [Versioned documentation](https://the-open-engine.github.io/zeroshot/)
- [Get started](docs/getting-started/first-run.md)
- [CLI reference](docs/zeroshot-cli.md)
- [Standalone HTML CLI reference](docs/zeroshot-cli.html)
- [Distribution contract](docs/zeroshot-distribution.md)
- [Target image guide](docker/zeroshot-target/README.md)
- [Python SDK](sdks/python/README.md)
- [Cluster API reference](https://the-open-engine.github.io/zeroshot/dev/reference/cluster/api/)
- [OpenEngine graph contract](docs/reference/cluster/graph.md)

## Development

```bash
npm ci
npm run check
cargo test --workspace
```

Node.js is repository tooling and the npm delivery mechanism only. See
[CONTRIBUTING.md](CONTRIBUTING.md), [PUBLISHING.md](PUBLISHING.md), and [SECURITY.md](SECURITY.md).

## License

MIT. See [LICENSE](LICENSE).
