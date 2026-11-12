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

Instances must be reachable over HTTPS with certificate verification enabled. Local delivery honors
the invoking shell's `HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY`, `NO_PROXY`, their lower-case forms,
`SSL_CERT_FILE` and `SSL_CERT_DIR`. Git transport, the early forge check and delivery use the same
captured settings, including after `resume`; no stored network configuration is needed. GitLab
accepts private CA certificates from the named PEM file or certificate directories. On Linux the
GitHub CLI also honors those CA paths; on macOS and Windows it uses the system certificate store,
so install your forge's CA there. An unreadable certificate file is reported by name. Proxy
credentials are kept private.
Relative CA paths are anchored to the invoking directory before a delivery worktree is created.
GitLab also supports `socks5://` and `socks5h://` proxy URLs through these same variables.

Older or unidentifiable servers fail version checks. Use a web base without a trailing slash, including any GitLab installation
prefix (such as `https://gitlab.example/install`). GitHub uses a root web URL.

## Local setup

Before agents start, local `--pr` and `--ship` runs make one authenticated, read-only forge request
to check connectivity, token acceptance, project readability and the supported server version.
A refusal names the forge, failed step and reason. Resume repeats this check with the resuming
shell's captured settings. Token scopes and merge permissions are checked by the actual operations.
Checkpoint resumes check after restoring the selected files, so delivery programs in the checkpoint
are available before the check and before any agent starts.
Runs without forge delivery and `--validate-only` keep their existing behavior.

Local `--push`, `--pr` and `--ship` runs require a clean checkout before agents start. Commit, stash
or remove staged, unstaged and unignored untracked changes named in the refusal. Finish or abort a
pending merge first. Changed submodule commit pointers count; files inside submodules do not.
A plain `zeroshot resume` does not repeat this check: the successor works on the checkout as it is,
so changes you make after a failure can be delivered with the run's own work. Runs without Git
delivery keep their normal behavior. Delivery shows Git's warning when a run adds an unregistered
embedded repository, but permits the gitlink; register it as a submodule when others need to clone it.

**Breaking change:** in-place delivery commits on the branch you checked out and pushes the
remote branch with that same name, including earlier unpushed commits. It creates no
`zeroshot/v2-…` branch. `--pr` and `--ship` open the PR/MR from that branch into a different
upstream branch when configured, otherwise the remote's default branch. Starting on the target
itself is refused before agents start: use a feature branch or `--worktree`. Explicit `--push`
may publish `main`. Delivery never force-pushes; non-fast-forward and forge policy refusals name
the branch and stop without an agent repair round.

Delivery can publish existing committed work without creating another commit and confirm that same
published commit again on resume. Before a fresh
PR/MR publication, it reconciles the target and sends an updated candidate through review,
including when the feature branch already exists on the remote.
Runs without a bound review reuse an unambiguous active review or open a new one, including
resumes before any delivery was recorded. Closed and merged reviews on a reused feature branch
belong to earlier runs. Bound review IDs retain terminal-review safeguards.
Resume recovers the current review's identity from the run lineage's
recorded delivery, including a confirmed review whose target reconciliation required repair,
timed out or was cancelled,
so older reviews on that branch do not make recovery ambiguous. A committed
candidate can still incorporate a diverged target without requiring another edit first; the integrated
result returns through review before publication.

For isolation, add `--worktree`. zeroshot creates a managed run branch in a retained worktree
and commits and pushes that matching branch. Its PR/MR targets the invoking branch's upstream,
or its own name without an upstream. The default base is your exact local HEAD, including
unpushed commits. To start from the upstream instead, use `--worktree --worktree-base remote`;
configure an upstream first, or explicitly choose `--worktree-base local`.

The invoking checkout's files, index, HEAD and branch tips stay unchanged. The new worktree
contains committed files only: dependencies, `.env` and other untracked resources are not copied,
linked or installed. Resume reuses the same workspace and attached run branch, while preserving
shared branch tips. If another checkout owns the saved branch, release it there before resuming.
Older local Git-delivery records without the working branch cannot resume safely. Preserve the
retained workspace, return to the intended attached branch, and start a new run from a feature
branch or with `--worktree`; zeroshot refuses recovery before restoring files or starting agents.
Worktrees remain after success and failure; inspect and remove them explicitly
with `git worktree remove PATH`. Use `git worktree list` from the invoking repository to find its
path. Hosted and named-target delivery keep their managed run branches.

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

Delivery checks unresolved review conversations before requesting merge. Human feedback requires
fresh repository write/admin access on GitHub or active Developer-or-higher membership on GitLab.
GitHub App bot feedback is authorized by GitHub's API actor type and stable identity on the bound
repository when posted; removing the app later does not invalidate it. App actions attributed to a
human follow that human's permission rule. Names, bot suffixes and comment text confer no authority.

Authorized findings return through the existing worker and both reviewers. The worker supplies each
addressed conversation's exact ID and revision, a disposition (`addressed` or `declined`) and a concise
response. After both reviewers accept, trusted delivery posts the response and resolves only that
unchanged conversation, then confirms resolution with the forge. Edits or new replies require
reassessment. Forwarding a finding once never counts as resolving it. Unauthorized blocking
conversations require a repository maintainer; delivery explains the blocker without dispatching
repair or resolving it. Interrupted confirmation retains the exact reply receipt and approved
revision: delivery rechecks resolved conversations too and confirms reopening any changed thread
before repair can proceed. Failed receipt persistence or confirmation keeps resolution and merge blocked. Ignoring PR feedback suppresses
automated repair and responses while retaining these conversation gates and all forge protections.
Earlier verified conversation revisions remain checked during later repairs, including CI repairs
whose worker returns no conversation responses.

Trusted execution checkpoints preserve the exact reply and verified conversation revision across
controller recovery, including when a later CI repair returns an empty response list. Restarting or
rewinding graph work retains the latest owned conversation evidence, even with workspace
snapshots disabled. Before posting a reply, delivery waits for durable acknowledgement of its
authenticated author, unique response hash and approved conversation revision. It records the reply ID
and resolution confirmation as the effects settle. If the forge accepts
the reply but loses its acknowledgement, delivery reconciles that exact reply; it does not post again
or ask the worker to repair its own response. Missing or ambiguous publication evidence blocks delivery
for a maintainer to reconcile. A definitive authentication rejection retires the unposted intent
before delivery refreshes credentials and retries. These receipts
remain recoverable while delivery waits on CI or approvals. Delivery rechecks these retained conversations before feedback and merge
admission. Changes discovered there are durably acknowledged before delivery continues or requests
a merge, including partial settlement when a forge read fails. Failed acknowledgement preserves the
previous receipts, including when removing the last conversation, so subsequent feedback and merge
attempts must retry that checkpoint. Recovery reconciles receipts acknowledged before a runtime or controller failure before
starting a successor. Compact receipt storage is bounded across repair rounds; reaching its limit stops
further automated replies and resolution with a maintainer intervention diagnostic.

When a reviewed response is replayed after recovery, an unrecognized resolved revision requires
reassessment through the same repair loop.

A GitHub merge rejection returns to conversation repair only when its complete forge response names
that blocker and a fresh read confirms it. Unknown API/schema failures retain their normal handling.

Delivery failures name the project, source and target branches, and the review and head revision
when known. They also list the last confirmed publication, so a failed `--ship` can still identify
the branch and PR/MR already available on the forge. A closed review must be reopened with its
original branches before resuming. An already merged review can confirm delivery in merge mode
with a clean workspace at its exact reviewed head; remaining work needs a new run from the target
branch. When GitLab rejects a merge with HTTP 401 but a fresh authenticated read succeeds, delivery
reports missing merge permission; grant the token that permission before resuming.

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
An existing MR without generated markers keeps its description and gains an appended generated section.
Refresh replaces only the generated markers' contents and preserves surrounding human text; incomplete,
duplicated or reversed markers are refused. A draft
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

GitLab can omit both merge and squash commit SHAs after a fast-forward merge. Delivery confirms
completion by proving that the exact reviewed head is on the target branch, including when the
target has advanced again. Missing evidence keeps the merge unconfirmed.

Resuming accepts intentional commits pushed to the run branch. If the complete restored candidate
already matches its published tree and no merge is pending, delivery reuses that commit without another
work round. Pending merges retain their normal reconciliation path. Different
local content is preserved and returns to the review loop. Discovery checks every review from the run
branch across bounded complete pages before validating its target; retargeted or ambiguous reviews
are refused rather than duplicated.
GitLab briefly rechecks a lagging merge-request head against the branch ref, including after
publishing an integration and before updating MR metadata. Persistent disagreement
stops with both revisions in the diagnostic instead of polling indefinitely.

If a worker rewrites history after publication, delivery refuses further publication. For a local
run, restoring a checkpoint replaces files and the index but keeps the attached branch at its
current tip. A checkpoint alone therefore cannot recover the published ancestry.

To continue the same local run and review without rewinding the branch, first preserve any work
you need by committing it on the run branch or saving it outside the checkout. Fetch the published
branch from its delivery remote, merge its head into the run's attached branch, and resolve and
commit any conflicts. Run the Git commands in the retained run workspace, using its managed
worktree for `--worktree` runs. Then select a checkpoint taken before the rewrite:

```console
git fetch REMOTE refs/heads/BRANCH
git merge --no-edit FETCH_HEAD
zeroshot checkpoints RUN_ID
zeroshot resume RUN_ID --from-checkpoint CHECKPOINT_ID --discard-workspace-changes
```

Use the actual delivery remote and branch for `REMOTE` and `BRANCH`. The merge preserves the
rewritten local commits and adds the published ancestry. The checkpoint restores its candidate
files and index while keeping that merged tip attached; it discards current tracked, staged,
untracked, and ignored files. If you cannot merge the histories, keep the old workspace for
inspection and start a new run on a new feature branch based on the fetched published head.
Neither procedure requires a force-push.

For a named target, use `zeroshot checkpoints RUN_ID --target NAME`, then
`zeroshot resume RUN_ID --from-checkpoint CHECKPOINT_ID --target NAME` with a checkpoint taken
before the rewrite. The target restores its own retained checkout; omit `--discard-workspace-changes`.
