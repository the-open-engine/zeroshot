FROM rust:1.97.0-trixie AS rust-toolchain
RUN rustup component add rustfmt clippy \
    && mkdir -p /opt/rustup \
    && cp -a /usr/local/rustup/. /opt/rustup/ \
    && cp -a /usr/local/cargo/bin /opt/rustup/bin

FROM rust-toolchain AS builder

WORKDIR /src

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY zeroshot ./zeroshot

RUN cargo build --locked --release --package zeroshot --bin zeroshot

FROM python:3.12.14-slim-trixie@sha256:78387bc3881b8273120a12ebe6c1ab22b018ccc2c9adf565ae1ac9b536e184ea AS python-runtime

FROM node:24-trixie-slim AS tooling

ARG CODEX_VERSION=0.148.0
ARG CLAUDE_CODE_VERSION=2.1.237
ARG NPM_VERSION=11.19.1
ARG ZEROSHOT_VERSION=development
ARG VCS_REF=unknown

LABEL org.opencontainers.image.source="https://github.com/the-open-engine/zeroshot" \
      org.opencontainers.image.title="Zeroshot target" \
      org.opencontainers.image.version="${ZEROSHOT_VERSION}" \
      org.opencontainers.image.revision="${VCS_REF}"

RUN apt-get update \
    && apt-get install --yes --no-install-recommends \
        bash \
        build-essential \
        ca-certificates \
        gh \
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
    && rm -rf /var/lib/apt/lists/*

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
        "@openai/codex@${CODEX_VERSION}" \
        "@anthropic-ai/claude-code@${CLAUDE_CODE_VERSION}" \
    && npm cache clean --force

FROM tooling AS runtime

COPY --from=builder /src/target/release/zeroshot /usr/local/bin/zeroshot
COPY docker/zeroshot-target/entrypoint.sh /usr/local/bin/zeroshot-target-entrypoint

# The native runtime deliberately retains root so it can isolate node processes.
# Root owns target state; execute-only access lets isolated run UIDs reach their own workspaces.
RUN install -d -m 0711 /var/lib/zeroshot/native-v2 \
    && chmod 0755 /usr/local/bin/zeroshot-target-entrypoint

VOLUME ["/var/lib/zeroshot"]
EXPOSE 8080

ENTRYPOINT ["zeroshot-target-entrypoint"]
CMD ["target", "serve", "--listen", "0.0.0.0:8080", "--public-origin", "http://127.0.0.1:8080", "--storage", "/var/lib/zeroshot/native-v2"]
