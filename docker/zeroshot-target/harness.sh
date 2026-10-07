#!/bin/sh
set -eu

# Project toolchains may change PATH; the harness keeps its image-owned interpreter.
name=${0##*/}
case "$name" in
  codex | copilot)
    exec /opt/zeroshot/bin/node "/opt/zeroshot/harness/bin/$name" "$@"
    ;;
  claude)
    exec /opt/zeroshot/harness/bin/claude "$@"
    ;;
  *)
    echo "unknown Zeroshot harness: $name" >&2
    exit 64
    ;;
esac
