# syntax=docker/dockerfile:1

FROM node:24-trixie-slim AS ui-builder

WORKDIR /src
COPY ui/package.json ui/package-lock.json ./ui/
RUN npm --prefix ui ci --ignore-scripts
COPY ui ./ui
RUN npm --prefix ui run build

FROM rust:1.97.0-trixie AS rust-toolchain
RUN rustup component add rustfmt clippy \
    && mkdir -p /opt/rustup \
    && cp -a /usr/local/rustup/. /opt/rustup/ \
    && cp -a /usr/local/cargo/bin /opt/rustup/bin

FROM golang:1.27.1-trixie@sha256:9baa6b4187bbb98d240372a8a235ac0bb6b5ddd52bba1431dc2f7c0705862728 AS restic-builder
ARG TARGETARCH
ENV CGO_ENABLED=0 GOTOOLCHAIN=local GOFLAGS=-p=4
COPY scripts/distribution/build-restic.sh /usr/local/bin/build-restic
RUN case "$TARGETARCH" in \
      amd64) target=x86_64-unknown-linux-musl ;; \
      arm64) target=aarch64-unknown-linux-musl ;; \
      *) echo "unsupported Restic architecture: $TARGETARCH" >&2; exit 2 ;; \
    esac \
    && build-restic /out "$target" \
    && cp "/out/$target/restic" /out/restic \
    && /out/restic version

FROM rust-toolchain AS builder

WORKDIR /src

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY zeroshot ./zeroshot
COPY --from=ui-builder /src/ui/dist ./ui/dist

RUN cargo build --locked --release --package zeroshot --bin zeroshot --features ui

FROM python:3.12.14-slim-trixie@sha256:78387bc3881b8273120a12ebe6c1ab22b018ccc2c9adf565ae1ac9b536e184ea AS python-runtime

FROM node:24-trixie-slim AS tooling

ARG COPILOT_VERSION=1.0.86
ARG CODEX_VERSION=0.155.0
ARG CLAUDE_CODE_VERSION=2.1.276
ARG TARGETARCH
ARG GH_VERSION=2.101.0
ARG NPM_VERSION=11.19.1
ARG ZEROSHOT_VERSION=development
ARG VCS_REF=unknown

LABEL org.opencontainers.image.source="https://github.com/the-open-engine/zeroshot" \
      org.opencontainers.image.title="Zeroshot target" \
      org.opencontainers.image.version="${ZEROSHOT_VERSION}" \
      org.opencontainers.image.revision="${VCS_REF}"

# Verify the upstream package independently of Debian's GitHub CLI version.
ADD --checksum=sha256:f8bbc37fc5568a6a162d1a67b1e9c1afa9139f7b5a46dcde4a57bdaa0db33b60 \
    https://github.com/cli/cli/releases/download/v${GH_VERSION}/gh_${GH_VERSION}_checksums.txt /tmp/github-cli/checksums.txt
ADD https://github.com/cli/cli/releases/download/v${GH_VERSION}/gh_${GH_VERSION}_linux_${TARGETARCH}.deb /tmp/github-cli/

RUN cd /tmp/github-cli \
    && sha256sum --check --ignore-missing checksums.txt \
    && apt-get update \
    && apt-get upgrade --yes \
    && apt-get install --yes --no-install-recommends \
        bash \
        build-essential \
        ca-certificates \
        /tmp/github-cli/gh_${GH_VERSION}_linux_${TARGETARCH}.deb \
        git \
        libbz2-1.0 \
        libffi-dev \
        libgdbm6t64 \
        liblzma5 \
        libncursesw6 \
        libreadline8t64 \
        libsqlite3-0 \
        libssl-dev \
        libuuid1 \
        pkgconf \
        ripgrep \
    && git_version="$(git --version)" \
    && dpkg --compare-versions "${git_version#git version }" ge 2.47 \
    && rm -rf /var/lib/apt/lists/* /tmp/github-cli

COPY --from=python-runtime /usr/local /usr/local
COPY --from=rust-toolchain /opt/rustup /opt/rustup
COPY --chmod=0755 docker/zeroshot-target/rust-tool.sh /usr/local/libexec/zeroshot-rust-tool
RUN for tool in cargo cargo-clippy cargo-fmt clippy-driver rustc rustdoc rustfmt rustup; do \
        ln -s /usr/local/libexec/zeroshot-rust-tool "/usr/local/bin/$tool"; \
    done \
    && ldconfig

RUN npm install --global --no-audit --no-fund "npm@${NPM_VERSION}" \
    && npm install --global --no-audit --no-fund \
        --strict-allow-scripts \
        --allow-scripts=@anthropic-ai/claude-code \
        "@github/copilot@${COPILOT_VERSION}" \
        "@openai/codex@${CODEX_VERSION}" \
        "@anthropic-ai/claude-code@${CLAUDE_CODE_VERSION}" \
    && npm cache clean --force

FROM tooling AS runtime

COPY --from=builder /src/target/release/zeroshot /usr/local/bin/zeroshot
COPY --from=restic-builder /out/restic /usr/local/bin/restic
COPY docker/zeroshot-target/entrypoint.sh /usr/local/bin/zeroshot-target-entrypoint

# The native runtime deliberately retains root so it can isolate node processes.
# Root owns target state; execute-only access lets isolated run UIDs reach their own workspaces.
RUN install -d -m 0711 /var/lib/zeroshot/native-v2 \
    && chmod 0755 /usr/local/bin/zeroshot-target-entrypoint

VOLUME ["/var/lib/zeroshot"]
EXPOSE 8080

ENTRYPOINT ["zeroshot-target-entrypoint"]
CMD ["target", "serve", "--listen", "0.0.0.0:8080", "--public-origin", "http://127.0.0.1:8080", "--storage", "/var/lib/zeroshot/native-v2"]
