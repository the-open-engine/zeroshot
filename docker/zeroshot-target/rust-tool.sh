#!/bin/sh
set -eu

# Keep explicit or existing user installations authoritative; otherwise use the image baseline.
if [ -z "${RUSTUP_HOME+x}" ] && [ ! -d "$HOME/.rustup" ]; then
  export RUSTUP_HOME=/opt/rustup
fi
exec "/opt/rustup/bin/${0##*/}" "$@"
