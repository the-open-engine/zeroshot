# Zeroshot target image

`ghcr.io/the-open-engine/zeroshot-target` is the canonical self-hosted target server image. It
contains the native `zeroshot` executable plus pinned Codex, Claude, GitHub Copilot, and Pi
harness CLIs.
The image uses Debian Trixie with package updates applied at build time. GitHub CLI is pinned
separately to an upstream release, verified by checksum, and tested for delivery API pagination
before publication.

Coding tools include Node.js 24 and npm, Python 3.12 with pip, venv and extension headers,
Rust 1.97 with Cargo, rustfmt and Clippy, and C/C++ compilers, make, pkgconf, OpenSSL and
libffi development files. CI compiles and executes native fixtures with these tools as an
isolated user, with a read-only root filesystem, fresh home and no network.

The image's Rust installation is read-only. Explicit `RUSTUP_HOME` settings and existing
`~/.rustup` installations take precedence; Cargo caches stay in the agent's private home unless
it specifies `CARGO_HOME`. Other compiler versions and project dependencies remain caller-owned.
The harness CLIs and their Node interpreter live under `/opt/zeroshot`; changing
the project Node version or `PATH` does not change the harness interpreter.

## Run

The image runs an unauthenticated direct target:

```bash
docker run --detach --restart unless-stopped --name zeroshot-target \
  -p 127.0.0.1:8080:8080 \
  -v zeroshot-data:/var/lib/zeroshot \
  ghcr.io/the-open-engine/zeroshot-target:latest
```

The target is a long-running server: finishing a run leaves the container running.
`--restart unless-stopped` restarts it after an unexpected exit or when Docker starts again,
including after a reboot. Docker must itself be running. A manual `docker stop zeroshot-target`
keeps it stopped until you run `docker start zeroshot-target`.

The named `zeroshot-data` volume preserves profiles and run history when the container is replaced. If you
previously used `--rm` and the container disappeared, repeat the command above with the same volume.
Use `docker ps -a --filter name=zeroshot-target` to check whether the container exists and is running.

The container listens on port `8080` internally. Open `http://127.0.0.1:8080/ui/` for the profile
editor and live or recorded run history. The UI and target API share the same process and port;
there is no frontend server to start. With the loopback-only host publish above, register the target:

```bash
zeroshot target add local --url http://127.0.0.1:8080 --direct
```

Closing the browser leaves runs active.
Stopping the container stops its runtime; on restart, interrupted runs are reconciled as lost,
not restarted automatically. When recovery data is available, start a successor with
`zeroshot resume RUN_ID --target local`. Completed histories remain readable. Private cloud target
containers do not expose the unauthenticated standalone UI.

If the browser uses another port or an HTTPS reverse proxy, set `--public-origin` to that exact
origin. For example:

```bash
docker run --detach --restart unless-stopped --name zeroshot-target \
  -p 127.0.0.1:8185:8080 -v zeroshot-data:/var/lib/zeroshot \
  ghcr.io/the-open-engine/zeroshot-target:latest \
  target serve --listen 0.0.0.0:8080 --public-origin http://127.0.0.1:8185 \
  --storage /var/lib/zeroshot/native-v2
```

By default, the target performs no application-level authentication. Anyone who can reach the
published port can use it. Run the container only on a private machine and private network, and do
not expose the port to the public internet.

## Environment scripts and Docker

A run submission may supply `environment` beside `runtime`, with `setup`, `startup`, and ordinary
`variables`. Profiles have no environment field. With the CLI, pass the definition
as `--environment FILE`; see the [worked example](../../docs/guides/runtime-environments.md#example-pinned-node-and-npm-dependencies).
Setup runs as root before checkout or checkpoint restoration.
Startup runs as the workspace user after checkout or restoration and before any agent.
Both run again on resume, so startup must be idempotent with respect to workspace files.
Workers and reviewers share this prepared workspace and its dependencies. Setup must not
start root services or daemonize: its cleanup covers the shell process group, and trusted
root scripts can escape that group. Debian package service autostart is disabled.

Install shared project tools into `$ZEROSHOT_TOOLS`; its `bin` directory is already on
agents' `PATH`. Install repository dependencies in the workspace. Shell `export` commands
inside a hook affect that hook only; use declared variables for later agents. Keep
long-lived project services in startup. Background processes in the target stop with the run
and restart on resume. Detached Docker containers belong to the supplied daemon; a direct
target operator or startup script must manage their names, reuse, and cleanup. Checkpoints
restore workspace files and graph progress, not running processes or Docker data.

The direct target is an operator-trusted server. Root setup changes the target container,
including OS packages shared by other runs. Use a dedicated target container when projects
need different system environments. Replacing the target resets its root filesystem.

The image includes Docker CLI, Buildx and Compose. To use Docker, provide access to a
daemon, for example by mounting a socket with permissions that allow the run's workspace
user to connect. A mounted daemon controls the machine hosting it; use a disposable or
dedicated daemon. Workspace bind mounts in sibling containers require the workspace to
exist at the same absolute path on that daemon's host. Cloud supplies this arrangement
inside its disposable run machine.

## Build

Build from the repository root. The image build compiles and embeds the UI assets:

```bash
docker build --pull -f docker/zeroshot-target/Dockerfile -t zeroshot-target:dev .
```

Release builds set `ZEROSHOT_VERSION` and `VCS_REF` labels and are published only by
`.github/workflows/release.yml`.

For HTTPS, place a reverse proxy in front of the target and forward WebSocket upgrades on
`/native-v2/oecp`.
