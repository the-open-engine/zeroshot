#!/usr/bin/env bash
# Runs the stale-worktree checkpoint regression from a clean copy of this checkout in disposable
# containers: the rust-toolchain.toml Rust release, CI's Go release for the pinned Restic sidecar
# that local checkpoints require, and an init process that reaps the detached local controller.
# Run from the repository root: bash zeroshot/tests/native_v2_checkpoint_stale_worktree/run.sh
set -euo pipefail

readonly RUST_IMAGE='rust:1.97.0-bookworm@sha256:8fa55b2f3ddf97471ab6a767bfa3f37e6bad0986ba823e75fea57e2a2a5c3073'
readonly GO_IMAGE='golang:1.27.1-bookworm@sha256:8d48e12ec56735e9358640898b9d9b9fcca110612ed8a5567438c0a1baa24e66'
readonly TEST_TARGET=native_v2_checkpoint_stale_worktree
readonly TEST_NAMES='nested_worktree_of_a_moved_checkout_starts_or_names_its_path nested_worktree_moved_with_mv_starts_or_names_its_path workspace_that_is_a_moved_linked_worktree_starts_or_names_its_path'

checkout="$PWD"
if [[ ! -f "$checkout/rust-toolchain.toml" || ! -f "$checkout/zeroshot/tests/$TEST_TARGET.rs" ]]; then
  printf 'run from the repository root\n' >&2
  exit 2
fi
case "$(docker version --format '{{.Server.Arch}}')" in
  amd64) restic_target=x86_64-unknown-linux-musl ;;
  arm64) restic_target=aarch64-unknown-linux-musl ;;
  *) printf 'unsupported Docker server architecture\n' >&2; exit 2 ;;
esac

suffix="$$-$(date +%s)-$RANDOM"
restic_container="zeroshot-stale-worktree-restic-$suffix"
test_container="zeroshot-stale-worktree-test-$suffix"
scratch="$(mktemp -d)"
cleanup() {
  docker rm --force "$restic_container" "$test_container" >/dev/null 2>&1 || true
  rm -rf -- "$scratch"
}
trap cleanup EXIT

tar -C "$checkout" -cf - scripts/distribution/build-restic.sh |
  docker run --rm --interactive --name "$restic_container" "$GO_IMAGE" bash -c "
    set -euo pipefail
    mkdir /source
    tar -xf - -C /source
    bash /source/scripts/distribution/build-restic.sh /restic $restic_target >&2
    tar -C /restic/$restic_target -cf - restic
  " >"$scratch/restic.tar"

mkdir -p "$scratch/sidecar/target/debug"
tar -xf "$scratch/restic.tar" -C "$scratch/sidecar/target/debug"
tar -C "$checkout" --exclude=./.git --exclude=./target --exclude=node_modules \
  -cf "$scratch/source.tar" .
tar -C "$scratch/sidecar" -rf "$scratch/source.tar" target/debug/restic

docker run --rm --interactive --init --name "$test_container" "$RUST_IMAGE" bash -c "
  set -euo pipefail
  mkdir /work
  tar -xf - -C /work
  cd /work
  target/debug/restic version
  cargo test --locked --package zeroshot --test $TEST_TARGET -- --exact $TEST_NAMES
" <"$scratch/source.tar"
