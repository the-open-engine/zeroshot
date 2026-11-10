---
description: 'Submit a stack of dependent runs to a hosted target and deliver it by merging each run or as a chain of stacked pull requests.'
---

# Submit a stack

A stack is a set of dependent runs submitted to a hosted target at once. It is atomic and
immutable: every run uses the same repository, base branch and hosted profile, the target assigns
every run ID at submission, and a stack cannot be edited or retried in place. Each run has its own
static input, and `needs` names the runs that must finish first. Dependencies gate when a run may
start; they never pass output from one run to another.

The manifest's `delivery` chooses how the stack lands:

- `merge`: each run merges into the base branch before the runs that need it start.
- `stacked_pull_requests`: each run opens a pull request against the branch of the run before it,
  so the stack arrives as a chain of pull requests for review. Nothing is merged.

Stacks run on hosted targets with GitHub.com repositories. There is no local stack execution.

## Write a manifest

Save the manifest as JSON, for example `checkout.stack.json`:

```json
{
  "schema": "zeroshot.stack/v1",
  "title": "Checkout update",
  "delivery": "stacked_pull_requests",
  "source": { "repository": "acme/shop", "branch": "main" },
  "profile": "org:software-change",
  "expiresAt": "2026-11-10T00:00:00Z",
  "runs": {
    "api": { "input": { "task": "Add the order total endpoint." } },
    "client": {
      "needs": ["api"],
      "input": { "task": "Show the order total on the checkout page." }
    },
    "docs": {
      "needs": ["api"],
      "input": { "task": "Document the order total endpoint." }
    }
  }
}
```

| Field         | Meaning                                                                                                                                |
| ------------- | -------------------------------------------------------------------------------------------------------------------------------------- |
| `schema`      | Always `zeroshot.stack/v1`                                                                                                             |
| `title`       | Stack title; each run is titled `<title>: <run name>`                                                                                  |
| `delivery`    | Required: `merge` or `stacked_pull_requests`                                                                                           |
| `source`      | The repository (`owner/name`) and base branch every run uses                                                                           |
| `profile`     | A hosted profile, `user:NAME` or `org:NAME`, shared by every run                                                                       |
| `expiresAt`   | RFC 3339 deadline for the whole stack, in the future and at most seven days away                                                       |
| `runs`        | 1 to 64 runs keyed by name; each has an `input` matching the profile's graph input and optional `needs`, which must not form a cycle   |
| `environment` | Optional [runtime environment](runtime-environments.md) applied to every run; omit it to let the host choose each repository's default |

The profile decides how each run is built and delivered, so it must match `delivery`:

- `merge` needs a profile with exactly one Git merge delivery node (`merge@2` or `merge@3`), such
  as the `software-change` template with merge delivery.
- `stacked_pull_requests` needs a profile with exactly one pull-request delivery node (`pr@2`),
  such as the `software-change` template with pull-request delivery.

In both modes, agent bindings must not declare `GH_TOKEN`; only the Git delivery binding may.

## Validate, submit and follow a stack

Validate the manifest locally before submitting it. Validation checks the manifest, its deadline and
its dependencies without contacting a target:

```bash
zeroshot stack validate checkout.stack.json
```

Submit it to a hosted target you are logged in to. The submission key makes a retried submission
return the same stack instead of creating a second one:

```bash
zeroshot stack submit checkout.stack.json --target platform --submission-key checkout-1
```

Submission also checks the profile against `delivery` and each run's input against the profile's
graph, then prints the stack and polls it, printing each changed snapshot as a JSON line until the
stack finishes. Ctrl-C or `--detach` stops following without stopping the stack. Use the printed
`stackId` to follow or stop it later:

```bash
zeroshot stack status STACK_ID --target platform
zeroshot stack watch STACK_ID --target platform
zeroshot stack force-stop STACK_ID --target platform
```

`force-stop` stops every run that has not finished. Each run in the status has its `runId` for
`zeroshot status`, `zeroshot logs` and the other run commands, its `needs`, and the `sourceBranch`
and `sourceRevision` it started from once it materializes. A run starts only after every run it
needs succeeded; if one fails, the runs that need it end with the error code `dependency_failed`.
Once a run is ready, its queue deadline is the earlier of `expiresAt` and seven days later. See the
[CLI reference](../zeroshot-cli.md) for every field and flag.

## Merge delivery

With `"delivery": "merge"`, runs follow `needs` exactly, and a run may need several runs. A run
starts from the base branch once everything it needs has merged, so it builds on their changes.
Runs that do not depend on each other do not wait for each other. A merge conflict is repaired by
the conflicting run itself in its own checkout and delivery loop. The stack succeeds when every run
has merged.

## Stacked pull requests

With `"delivery": "stacked_pull_requests"`, the stack is always one line. When you validate or
submit it, zeroshot puts the runs in dependency order, breaking ties by run name, and replaces each
run's `needs` with just the run before it. A run therefore never starts before anything it needed.
`zeroshot stack validate` prints the order. For the manifest above:

```json
{ "valid": true, "order": ["api", "client", "docs"] }
```

`client` and `docs` both need only `api`, so they are ordered by name, and `docs` is stacked on
`client`. The first run starts from the base branch and opens a pull request against it. Each later
run starts from the branch of the run before it, which its status reports as `sourceBranch`, and
opens its pull request against that branch, so each pull request shows only its own run's changes.
A run's work is done when its pull request is open and its required checks pass; zeroshot does not
merge it. The stack is complete once every pull request is open.

### Review and merge the chain

Review and merge the pull requests from the bottom of the stack up. zeroshot and the hosted target
never retarget a pull request; GitHub does:

- When a parent pull request merges and its branch is deleted, GitHub moves the next pull request
  onto the base branch. Turn on the repository's "Automatically delete head branches" setting, or
  delete the branch after merging.
- If the parent was squash-merged, or merged by rebasing, the base branch has new commits rather
  than the parent's own, so the child pull request still lists the parent's commits. Rebase the
  child onto the base branch without them, where `PARENT_HEAD` is the parent pull request's last
  commit:

  ```bash
  git fetch origin
  git rebase --onto origin/main PARENT_HEAD CHILD_BRANCH
  git push --force-with-lease origin CHILD_BRANCH
  ```

- Changes pushed to a parent pull request during review are not restacked automatically. Rebase
  each pull request above it onto its updated parent by hand, from the bottom up, for example
  `git rebase origin/PARENT_BRANCH CHILD_BRANCH` followed by a force push.

## Plan a stack

The `zeroshot-plan` skill splits large work into plans whose `depends_on` lists the plans that
must finish first. When plans depend on each other, the skill offers to write a stack manifest to
`.zeroshot/plans/<id>.stack.json` with one run per plan, and leaves `delivery` for you to choose. See
[Plan work before running it](planning.md).
