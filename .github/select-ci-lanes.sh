#!/usr/bin/env bash
set -euo pipefail

# Missing history and manual dispatches select every lane. Never infer a docs-only
# change from an unavailable base, an empty diff, or a failed Git command.
if [[ "${EVENT_NAME:-}" == "workflow_dispatch" ]] ||
   [[ -z "${BASE_SHA:-}" ]] ||
   [[ "$BASE_SHA" =~ ^0+$ ]] ||
   ! git cat-file -e "${BASE_SHA}^{commit}" 2>/dev/null ||
   ! git cat-file -e "${HEAD_SHA:-}^{commit}" 2>/dev/null; then
  node .github/ci-path-classifier.js < /dev/null
  exit 0
fi

git diff --no-renames --name-only -z "$BASE_SHA" "$HEAD_SHA" |
  node .github/ci-path-classifier.js
