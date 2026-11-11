---
description: 'Set up GitHub and GitLab delivery with native CLI logins or token connections.'
---

# GitHub and GitLab delivery

GitHub.com, GitHub Enterprise, and GitLab use the same delivery nodes for PR/MR creation, merging,
and issue or review comments.

| Service                                     | Minimum version | API credential |
| ------------------------------------------- | --------------- | -------------- |
| GitHub.com / Enterprise Cloud (`*.ghe.com`) | Hosted service  | `GH_TOKEN`     |
| GitHub Enterprise Server                    | 3.22.2          | `GH_TOKEN`     |
| GitLab.com / self-managed GitLab            | 19.5.1          | `GITLAB_TOKEN` |

Instances must be publicly reachable over HTTPS with trusted TLS. Older or unidentifiable servers
fail version checks. Use a web base without a trailing slash, including any GitLab installation
prefix (such as `https://gitlab.example/install`). GitHub uses a root web URL.

## Local setup

Local `--push`, `--pr` and `--ship` runs require a clean checkout before agents start. Commit, stash
or remove staged, unstaged and unignored untracked changes named in the refusal. Finish or abort a
pending merge first. Changed submodule commit pointers count; files inside submodules do not.
Runs without Git
delivery keep their normal behavior. Delivery shows Git's warning when a run adds an unregistered
embedded repository, but permits the gitlink; register it as a submodule when others need to clone it.

GitHub delivery requires `gh`. GitLab needs `glab` only to reuse its native login. Sign in to the host you use:

```console
gh auth login --hostname github.com
gh auth login --hostname ghe.example
glab auth login --hostname gitlab.com
```

Local delivery reuses the selected host's login when its connection is empty. Explicit or saved
values take precedence; partial connections fail. Ambient tokens are bound to `GH_HOST` or
`GITLAB_HOST`; without a selector they apply only to GitHub.com or GitLab.com. Other hosts, including
`*.ghe.com`, require a matching selector, a per-host CLI login, or declared connection values.
Use `repo` and `read:org` for a classic GitHub PAT,
or `api` and `write_repository` for a GitLab PAT. Git fetch and push must also work.

GitHub.com and GitLab.com remotes identify themselves automatically. Other hosts need a configured
`gh`/`glab` login or explicit selection; a host configured in both CLIs needs explicit selection:

```console
zeroshot run --title "Example change" --input task.json \
  --template software-change --uniform-runtime-config runtime.json --pr \
  --source-provider source.gitlab --source-base-url https://gitlab.example/install
```

Use `source.github` for GitHub. The paired flags select the current remote's forge; they do not change
the remote. Supply them for GitLab installation prefixes.

## Docker and Platform

Contained runs need an exact Git source. Set `remote.forge` to the provider, repository and HTTPS
web base, separately from the Git locator:

```json
{
  "repository": "team/project",
  "branch": "main",
  "revision": "0123456789012345678901234567890123456789",
  "remote": {
    "url": "https://gitlab.example/install/team/project.git",
    "connection": "company-forge",
    "forge": {
      "provider": "source.gitlab",
      "repository": "team/project",
      "baseUrl": "https://gitlab.example/install"
    }
  }
}
```

The source connection supplies `GIT_USERNAME` / `GIT_PASSWORD` for HTTPS, or
`GIT_SSH_PRIVATE_KEY` / `GIT_SSH_KNOWN_HOSTS` for SSH. Declare the API token on the delivery node:

```json
{
  "kind": "git_delivery",
  "connections": { "company-forge": ["GITLAB_TOKEN"] }
}
```

Default API connections are `github.GH_TOKEN` and `gitlab.GITLAB_TOKEN`; explicit bindings keep their names.
Named targets read the declared field names for both submission and resume. Native CLI aliases such
as `GH_ENTERPRISE_TOKEN` and `GITLAB_ACCESS_TOKEN` apply only to local runs.
One static connection can hold both transport and API fields; inline run values work too. GitHub uses
`GH_TOKEN`. An API field alone does not authorize a private clone or push. Agents receive only the
connection fields their own bindings declare.

## Scope and limits

GitHub.com-only features include `--task-from`, Platform repository browsing and App/OAuth setup,
inbound commands, and hosted [stacks](stacks.md). For GHE or GitLab comments, supply the graph input
directly.

Before first publication, GitHub delivery reads merge-queue policy for the exact repository and
configured target branch. A confirmed queue owns integration with newer target commits, so delivery
publishes the reviewed candidate without a local target merge or a review round caused only by target
motion. Missing, malformed or failed policy reads never confirm a queue. Without a queue, delivery
fetches and integrates the exact current target and returns changed candidates through repair and review.
Push delivery and other forges retain their existing behavior.

Later GitHub gates refresh policy, required checks and reviews on the exact head. If the queue was
removed, current base/head ancestry routes an outdated head through ordinary authorized reconciliation,
even when nonstrict protection reports `CLEAN`. This also works after recovery without a saved queue
assumption. Adding a queue after ordinary integration permits queue admission only after that integration
has been reviewed. Deferred integration never advances `reviewBaseRevision`; `sourceRevision` remains
the admitted source. Queue admission still uses the exact reviewed head and success requires an observed
merged revision.

When the forge refuses the run-branch push because of branch protection or missing push permission,
delivery stops with a policy refusal naming the run branch and the forge's message, without another
agent round. Local work is preserved: let delivery push its run branch, then
[resume the run](observe-and-control.md#restart-or-resume-a-failed-run). Other push rejections,
including generic pre-receive hook messages, return to the repair loop.

Delivery preserves review identity across retries and confirms the integrated revision before
reporting a merge. Comment retries reuse the original publication; captured MR scopes refuse stale
base/head revisions. Required GitLab merge trains remain pending; synthetic merged-results
pipelines do not prove required CI for the captured head.

The GitLab adapter makes slash-leading generated prose literal with a Markdown backslash escape,
including endpoint descriptions such as `/health returns 200` and quick-action-looking text such as
`/close`. Existing escapes remain unchanged. The same conservative escape applies to slash-leading
lines inside code examples, without parsing Markdown or maintaining a command catalog. This normalization
also applies to comments before publication hashing and durable intent; retries reuse the same
write-once publication.

Generated GitLab MR titles lose leading `Draft:`, `[Draft]`, and `(Draft)` prefixes (case-insensitively,
including repeated prefixes). Empty results become `Zeroshot change`. Titles are limited to 255 Unicode
characters; shortened titles retain the complete original once in the generated description section.
The same title normalization runs before generated delivery commits, including repair commits pushed
to an existing MR, so GitLab's push processing cannot mark it draft from zeroshot's commit message.
Creation, refresh, and confirmation use the same canonical metadata, including for custom graphs.
Refresh replaces only the generated markers' contents and preserves surrounding human text. A draft
deliberately applied after creation keeps its existing title and draft status during refresh.

GitLab `--pr` readiness, human-review handoff and `--ship` merging use the same required-CI policy.
When the project's **Pipelines must succeed** setting (`only_allow_merge_if_pipeline_succeeds`) is
explicitly `false`, CI is `NotRequired`: an absent, pending, running, successful, failed, skipped or
canceled optional pipeline does not gate delivery. This never reports optional CI as `Passed`, and
projects without CI gain no new wait. When the setting is `true`, missing or running captured-head
CI stays pending, successful CI passes, failed CI follows the repair path, and skipped CI passes
only if the project explicitly allows it. Missing or unknown policy cannot bypass CI. Drafts,
conflicts, discussion policy, human approval for merging, and other forge gates still apply;
merge submission remains SHA-guarded and requires authoritative confirmation of the merged revision.

On fast-forward and semi-linear GitLab projects (`merge_method` `ff` or `rebase_merge`), a target
branch that advances after the MR opens makes GitLab report `need_rebase`. Delivery then fetches the
exact current target and merges it into the run branch locally, without asking GitLab to rebase, and
the changed candidate returns through repair and review before it is published again. If GitLab still
requires a rebase although the candidate already contains that target, delivery stops with a refusal
naming the merge method and target revision.

Resuming accepts intentional commits pushed to the run branch. If the complete restored candidate
already matches its published tree and no merge is pending, delivery reuses that commit without another
work round. Pending merges retain their normal reconciliation path. Different
local content is preserved and returns to the review loop. Discovery checks every review from the run
branch before validating its target; retargeted or ambiguous reviews are refused rather than duplicated.
GitLab briefly rechecks a lagging merge-request head against the branch ref. Persistent disagreement
stops with both revisions in the diagnostic instead of polling indefinitely.

If a worker rewrites local history after publication, delivery refuses further publication; list
checkpoints with `zeroshot checkpoints RUN_ID`, then run
`zeroshot resume RUN_ID --from-checkpoint CHECKPOINT_ID` using a checkpoint taken before the rewrite
to continue with the existing published branch and review.
