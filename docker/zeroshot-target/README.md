# Zeroshot target image

`ghcr.io/the-open-engine/zeroshot-target` is the canonical self-hosted target server image. It
contains the native `zeroshot` executable plus pinned Codex and Claude harness CLIs.

## Run

```bash
docker run --rm \
  -p 8080:8080 \
  -v zeroshot-data:/var/lib/zeroshot/native-v2 \
  ghcr.io/the-open-engine/zeroshot-target:latest
```

The default command serves an unauthenticated direct target at `http://127.0.0.1:8080`. Register it
with:

```bash
zeroshot target add local --url http://127.0.0.1:8080 --direct
zeroshot target setup local --repository owner/repository --branch main
```

## Private bootstrap

Set `ZEROSHOT_TARGET_BOOTSTRAP_KEY` to start the generic private bootstrap flow. The entrypoint writes
it to a mode-0600 temporary file, unsets the environment variable, passes the file to the target,
and removes it on exit. The target consumes the file while starting.

```bash
docker run --rm \
  -p 8080:8080 \
  -e ZEROSHOT_TARGET_BOOTSTRAP_KEY \
  -v zeroshot-data:/var/lib/zeroshot/native-v2 \
  ghcr.io/the-open-engine/zeroshot-target:latest
```

## Build

Build from the repository root:

```bash
docker build -f docker/zeroshot-target/Dockerfile -t zeroshot-target:dev .
```

Release builds set `ZEROSHOT_VERSION` and `VCS_REF` labels and are published only by
`.github/workflows/release.yml`.

For HTTPS, place a reverse proxy in front of the target and forward WebSocket upgrades on
`/native-v2/oecp`.
