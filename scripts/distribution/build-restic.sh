#!/usr/bin/env bash
set -euo pipefail

readonly RESTIC_VERSION=0.19.1
readonly RESTIC_SOURCE_SHA256=bb9b1a19040744d26d8a79be029d4e6b189c45ccc9d8831d7fe367d3c33df725
readonly OUTPUT_ROOT="${1:?usage: build-restic.sh OUTPUT_ROOT [TARGET ...]}"
shift

build_root="$(mktemp -d)"
trap 'rm -rf -- "$build_root"' EXIT
curl --fail --location --silent --show-error \
  "https://github.com/restic/restic/releases/download/v${RESTIC_VERSION}/restic-${RESTIC_VERSION}.tar.gz" \
  --output "$build_root/restic.tar.gz"
printf '%s  %s\n' "$RESTIC_SOURCE_SHA256" "$build_root/restic.tar.gz" | sha256sum --check -
mkdir "$build_root/source"
tar -xzf "$build_root/restic.tar.gz" --strip-components=1 -C "$build_root/source"
cd "$build_root/source"
go get \
  golang.org/x/crypto@v0.55.0 \
  golang.org/x/net@v0.58.0 \
  golang.org/x/text@v0.41.0 \
  google.golang.org/grpc@v1.83.2
go mod verify
go build -o "$build_root/restic-build" ./build.go

if [[ "$#" -eq 0 ]]; then
  set -- \
    x86_64-unknown-linux-musl \
    aarch64-unknown-linux-musl \
    x86_64-apple-darwin \
    aarch64-apple-darwin \
    x86_64-pc-windows-msvc
fi

for target in "$@"; do
  case "$target" in
    x86_64-unknown-linux-musl) goos=linux; goarch=amd64; executable=restic ;;
    aarch64-unknown-linux-musl) goos=linux; goarch=arm64; executable=restic ;;
    x86_64-apple-darwin) goos=darwin; goarch=amd64; executable=restic ;;
    aarch64-apple-darwin) goos=darwin; goarch=arm64; executable=restic ;;
    x86_64-pc-windows-msvc) goos=windows; goarch=amd64; executable=restic.exe ;;
    *) printf 'unsupported Restic target: %s\n' "$target" >&2; exit 2 ;;
  esac
  destination="$OUTPUT_ROOT/$target/$executable"
  mkdir -p "$(dirname "$destination")"
  "$build_root/restic-build" --goos "$goos" --goarch "$goarch" --output "$destination"
  chmod 0755 "$destination"
  metadata="$(go version -m "$destination")"
  for dependency in \
    $'golang.org/x/crypto\tv0.55.0' \
    $'golang.org/x/net\tv0.58.0' \
    $'golang.org/x/text\tv0.41.0' \
    $'google.golang.org/grpc\tv1.83.2'; do
    grep --fixed-strings --quiet "$dependency" <<<"$metadata"
  done
  printf 'built Restic %s for %s\n' "$RESTIC_VERSION" "$target"
done
