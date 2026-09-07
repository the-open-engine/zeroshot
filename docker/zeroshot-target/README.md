# Zeroshot target image

`ghcr.io/the-open-engine/zeroshot-target` is the canonical self-hosted target server image. It
contains the native `zeroshot` executable plus pinned Codex and Claude harness CLIs.

## Run

The image runs an unauthenticated direct target:

```bash
docker run --rm --detach --name zeroshot-target \
  -p 127.0.0.1:8080:8080 \
  -v zeroshot-data:/var/lib/zeroshot/native-v2 \
  ghcr.io/the-open-engine/zeroshot-target:latest
```

The container listens on port `8080` internally. With the loopback-only host publish above, register
it at `http://127.0.0.1:8080`:

```bash
zeroshot target add local --url http://127.0.0.1:8080 --direct
zeroshot target setup local --repository owner/repository --branch main
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
