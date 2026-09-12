# Zeroshot target image

`ghcr.io/the-open-engine/zeroshot-target` is the canonical self-hosted target server image. It
contains the native `zeroshot` executable plus pinned Codex and Claude harness CLIs.

Coding tools include Node.js 24 and npm, Python 3.12 with pip, venv and extension headers,
Rust 1.97 with Cargo, rustfmt and Clippy, and C/C++ compilers, make, pkgconf, OpenSSL and
libffi development files. CI compiles and executes native fixtures with these tools as an
isolated user, with a read-only root filesystem, fresh home and no network.

The image's Rust installation is read-only. Explicit `RUSTUP_HOME` settings and existing
`~/.rustup` installations take precedence; Cargo caches stay in the agent's private home unless
it specifies `CARGO_HOME`. Other compiler versions and project dependencies remain caller-owned.

## Run

The image runs an unauthenticated direct target:

```bash
docker run --rm --detach --name zeroshot-target \
  -p 127.0.0.1:8080:8080 \
  -v zeroshot-data:/var/lib/zeroshot \
  ghcr.io/the-open-engine/zeroshot-target:latest
```

The container listens on port `8080` internally. With the loopback-only host publish above, register
it at `http://127.0.0.1:8080`:

```bash
zeroshot target add local --url http://127.0.0.1:8080 --direct
```

By default, the target performs no application-level authentication. Anyone who can reach the
published port can use it. Run the container only on a private machine and private network, and do
not expose the port to the public internet.

## Build

Build from the repository root:

```bash
docker build -f docker/zeroshot-target/Dockerfile -t zeroshot-target:dev .
```

Release builds set `ZEROSHOT_VERSION` and `VCS_REF` labels and are published only by
`.github/workflows/release.yml`.

For HTTPS, place a reverse proxy in front of the target and forward WebSocket upgrades on
`/native-v2/oecp`.
