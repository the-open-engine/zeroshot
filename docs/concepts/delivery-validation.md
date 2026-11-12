---
description: 'Configure a caller-owned local executable to validate delivery before pushing, during PR review and before requesting a merge.'
---

# Caller-owned delivery validation

Local callers may select `ZEROSHOT_DELIVERY_CHECK_COMMAND` as one absolute private executable outside
of the checkout. Its path crosses the private controller bootstrap, never graph input or provider
environments. This is a trusted local integration point, not an OS isolation boundary against the
same user. Hosted profiles cannot select a local executable.

Before pushing the captured candidate (`publish`), immediately after a PR is synchronized (`published`),
while observing an open review (`review`), and immediately before requesting a merge (`merge`), delivery sends
`openengine.delivery-validation/v1` JSON on stdin. It includes the publication stage, repository,
branches, exact head and tree, trusted review base when available, and verifier-authored title and body.
No delivery credential is supplied. The helper returns exactly one verdict: `accepted`, `pending`,
`repair` with a public-safe reason, or `refused` with a public-safe reason. Pending keeps polling;
repair enters the authored repair/review loop; refusal, malformed output and process failure stop the
execution. Pending observation does not hide CI or new feedback; pending publication and merge block
those effects. Acceptance cannot override forge policy, required checks, reviews or merge queues.

The helper has no imposed turn deadline. Bounded pipes and cancellation use the normal process session;
its process tree must be confirmed gone before delivery releases the workspace, even if the outer
operation is cancelled. An accepted helper must leave the captured candidate clean and unchanged.
Forge confirmation remains authoritative: accepting or queuing a merge is not a merged receipt.

The published-stage notification precedes the first policy/status read, so a caller can durably record
the actual PR even when a subsequent inspection fails. Terminal merged/closed observations settle before
review-stage hooks or late feedback. A completed remote merge cannot become another repair.

## Example

This check runs the test suite on the exact candidate before it is pushed and again before the
merge request, in a scratch clone of that exact commit, so the checkout stays clean. It needs `jq`.
If any setup step fails, it exits non-zero, which stops the execution instead of accepting.

```sh
#!/bin/sh
# Run the tests on the exact candidate in a scratch clone, so the checkout stays clean.
# The check starts with only LANG=C, so it sets PATH and HOME itself.
# Any setup failure exits non-zero, which stops the execution instead of accepting.
set -eu
export PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin
export HOME=/Users/you # npm keeps its cache here

request=$(cat)
stage=$(printf '%s' "$request" | jq -r .stage)
head=$(printf '%s' "$request" | jq -r .headRevision)

case "$stage" in
  publish | merge) ;;
  *) echo '{"verdict":"accepted"}'; exit 0 ;;
esac

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
git clone --quiet --shared --no-checkout . "$scratch/repo"
git -C "$scratch/repo" checkout --quiet --detach "$head"
if (cd "$scratch/repo" && npm ci && npm test) >/dev/null 2>&1; then
  echo '{"verdict":"accepted"}'
else
  jq -cn --arg head "$head" '{verdict: "repair",
    reason: "npm test failed on \($head[0:7]). Run npm test in the checkout to see the failures."}'
fi
```

Install it outside the checkout, private to you, and point zeroshot at it:

```sh
mkdir -p ~/.local/bin
install -m 700 delivery-check.sh ~/.local/bin/zeroshot-delivery-check
export ZEROSHOT_DELIVERY_CHECK_COMMAND="$HOME/.local/bin/zeroshot-delivery-check"
```

It receives a request like this on stdin:

```json
{
  "kind": "openengine.delivery-validation/v1",
  "stage": "publish",
  "repository": "acme/shop",
  "targetBranch": "main",
  "headBranch": "zeroshot/run-4225",
  "headRevision": "3412f26ed5fb9ed72c59b582de15fe6d5e2be7b2",
  "tree": "8d1e5f0c4b2a7e9d3f6c1b0a5e8d2c7f4a9b3e6d",
  "reviewBaseRevision": null,
  "title": "Stop retried webhooks from booking twice",
  "description": "Deduplicates webhook deliveries by event id before booking.",
  "reviewId": null
}
```

and prints exactly one of these verdicts:

```json
{"verdict": "accepted"}
{"verdict": "pending"}
{"verdict": "repair", "reason": "npm test failed on 3412f26. Run npm test in the checkout to see the failures."}
{"verdict": "refused", "reason": "Changes under infra/ need a human owner."}
```

- The setting is one absolute path to an executable file outside the checkout, owned by you, with no
  group or other permissions (`chmod 700`). It is not a shell command, takes no arguments and cannot
  be a symlink.
- The check runs in the checkout. On macOS and Linux its environment holds only `LANG=C`, so it sets
  `PATH`, `HOME` and anything else it needs.
- It prints one JSON object of at most 1 MiB and exits 0, including for `repair` and `refused`. A
  non-zero exit, malformed JSON or an unknown field stops the execution.
- A `reason` is non-empty, at most 8 KiB, with no control characters other than newline and tab.
  Keep it public-safe.
- An accepting check leaves the checkout clean and unchanged.
