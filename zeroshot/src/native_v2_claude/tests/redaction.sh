#!/bin/sh
# Runs the Claude cross-block redaction regression from the repository root with the toolchain
# pinned in rust-toolchain.toml and the crates pinned in Cargo.lock. A host without Cargo runs the
# same command in the matching official Rust image, as the invoking user, inside this checkout.
set -eu

filter='native_v2_claude::tests::redaction::'
toolchain=$(sed -n 's/^channel = "\([^"]*\)"$/\1/p' rust-toolchain.toml)
test -n "$toolchain"
test -f zeroshot/src/native_v2_claude/tests/redaction.rs

if command -v cargo >/dev/null 2>&1; then
  exec cargo test --locked --package zeroshot --lib "$filter" -- --test-threads=1
fi

if ! command -v docker >/dev/null 2>&1; then
  echo "redaction regression setup: neither cargo nor docker is available" >&2
  exit 127
fi

exec docker run --rm \
  --name "zeroshot-claude-redaction-$$-$(date +%s)" \
  --user "$(id -u):$(id -g)" \
  --env CARGO_HOME=/work/target/claude-redaction-cargo-home \
  --env CARGO_TERM_COLOR=never \
  --volume "$(pwd):/work" \
  --workdir /work \
  "rust:${toolchain}-bookworm" \
  cargo test --locked --package zeroshot --lib "$filter" -- --test-threads=1
