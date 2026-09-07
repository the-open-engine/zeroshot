<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/brand/zeroshot-hero-dark.png">
  <img alt="Zeroshot. Self-driving software engineering. Layer 01 · Verification, The Open Engine." src="docs/brand/zeroshot-hero-light.png" width="100%">
</picture>

[![npm](https://img.shields.io/npm/v/%40the-open-engine-company%2Fzeroshot?style=flat&labelColor=171411&color=171411)](https://www.npmjs.com/package/@the-open-engine-company/zeroshot)
[![CI](https://img.shields.io/github/actions/workflow/status/the-open-engine/zeroshot/ci.yml?style=flat&labelColor=171411&label=CI)](https://github.com/the-open-engine/zeroshot/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-171411?style=flat)](LICENSE)

</div>

> [!IMPORTANT]
> **Zeroshot v8 is a hard interface cutover.** The former Node.js product, commands, configuration,
> state, and release train are discontinued; v8 does not provide aliases, migration, or compatibility
> shims for them.

# Zeroshot

Zeroshot runs durable multi-agent graph workloads locally or on named targets. The canonical product
is the native `zeroshot` executable in this repository.

## Install

The npm package downloads a verified release archive and exposes the canonical command:

```bash
npm install -g @the-open-engine-company/zeroshot
zeroshot version
```

The npm installer supports Linux x64/arm64, macOS x64/arm64, and Windows x64. Direct native archives
and `SHA256SUMS` are attached to each `vX.Y.Z` GitHub Release.

Python users install a platform wheel while importing the unchanged package name:

```bash
pip install the-open-engine-zeroshot
```

```python
from zeroshot import Client
```

The target server image is:

```text
ghcr.io/the-open-engine/zeroshot-target
```

## Run Locally

A run needs an input document plus an exact runtime plan or profile. For example:

```bash
cp docker/zeroshot-target/examples/software-change-input.json /tmp/zeroshot-input.json
cp docker/zeroshot-target/examples/claude-anthropic-runtime.json /tmp/zeroshot-runtime.json

zeroshot run \
  --title "Implement the requested change" \
  --template software-change \
  --input /tmp/zeroshot-input.json \
  --runtime-config /tmp/zeroshot-runtime.json
```

Foreground runs stream NDJSON until completion. Use `--detach` to return after submission.

```bash
zeroshot list
zeroshot status <run-id>
zeroshot watch <run-id>
zeroshot logs <run-id>
zeroshot force-stop <run-id>
```

Model IDs are provider-owned opaque strings. Zeroshot does not infer a harness from a model and does
not maintain model catalogs.

## Run a Target

Start the published target image:

```bash
docker run --rm -p 8080:8080 \
  -v zeroshot-data:/var/lib/zeroshot/native-v2 \
  ghcr.io/the-open-engine/zeroshot-target:latest
```

Register a direct target and its source defaults:

```bash
zeroshot target add local --url http://127.0.0.1:8080 --direct
zeroshot target setup local --repository owner/repository --branch main
```

Then add `--target local` to `zeroshot run`.

## Interfaces

- [CLI reference](docs/zeroshot-cli.md)
- [Standalone HTML CLI reference](docs/zeroshot-cli.html)
- [Distribution contract](docs/zeroshot-distribution.md)
- [Target image guide](docker/zeroshot-target/README.md)
- [Python SDK](sdks/python/README.md)
- [OpenEngine cluster protocol](docs/openengine-cluster-protocol/v1/graph-contract.md)

The CLI references are generated from the typed Clap command model. Regenerate them with:

```bash
cargo run -p zeroshot --example generate_cli_docs -- --write
```

## Development

```bash
npm ci
npm run check
cargo test --workspace
```

Node.js is repository tooling and the npm delivery mechanism only; it is not a second Zeroshot
runtime. See [CONTRIBUTING.md](CONTRIBUTING.md) and [PUBLISHING.md](PUBLISHING.md).

## License

MIT. See [LICENSE](LICENSE).
