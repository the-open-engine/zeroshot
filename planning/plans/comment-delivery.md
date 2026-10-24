# Comment delivery for issue and PR reviews

Status: native implementation rebased onto `origin/main`; final checks and release remain.
Platform integration is implemented and tested with external local native dependency patches.
Its production runtime/SDK pins and deployment remain unchanged until the native release exists.

## Accepted architecture

`--task-from URL` prepares ordinary `{url, context}` input before graph execution; `--input FILE`
still passes caller JSON unchanged. The URL is a required input, not submission metadata or a
`subject` object. Workers have no mandatory `task` field. Optional `instructions` supply caller guidance.

Intake captures title/body and exact source. Issue context has `reviewScope: {}`; PR context requires
complete base/head repository, branch and revision identities plus `linkedIssues: [{url,title,body}]`.
One bounded GraphQL snapshot captures formal closing relationships, including manual links, with
at most 100 issues. Incomplete responses fail preparation; absent referenced text remains unverified.
Fetched text is untrusted. Plain mentions do not count as formal linked issues.

Local review requires clean tracked files and the selected PR head; it never switches or resets
the worktree. Shared Git transport fetches captured head/base objects, including fork PR refs.
Local fetch/ref writes occur under the controller workspace lease after checkpoint restoration.
Explicit submission-key retries preserve prepared input/source and authored configuration within
the selected target/login/organization authority. Resume reuses admitted input.

`builtin.comment-delivery.post@1` is a trusted native verifier with a `comment_delivery` binding.
It requires `{url, body}` and optionally receives `reviewScope`; it has no model or provider lane.
Admission freezes destination/scope from required caller bindings and allows one publisher after
all other executions and cleanup, outside loops/maps, with no later executable work.
Publication rechecks captured PR scope. Generic local comment graphs need no agent or Git source.

The existing source registry selects forge authority independently of Git transport. Credentials
stay inside the native adapter. Durable intent precedes POST; confirmation precedes the receipt.
Publication storage lives outside workspace/checkpoint rollback and follows root recovery lineage.
Matching retries reuse the confirmed comment; changed content refuses. An uncertain POST reconciles
exact target, marker, author and body; unresolved publication never authorizes a blind repost.
Terminal success contains the exact nested `receipt`, including its body hash and permalink.

Platform supplies scoped publication storage through `HostedWorkspace.comment_publications` and
resolves metadata with host-owned connection authority. Direct requests carry explicit temporary
GitHub authority. Targets advertise separate comment-delivery and task-resolution capabilities.
Platform's lifecycle comments remain separate; the native engine posts the final report.

## Current graphs and defaults

`pr-review` runs a classifier verifier, a zero/one reproducer step, and a code-checker verifier,
then the native publisher. Classifier output is `reproductionTasks: [{plan}]`; `[]` skips reproduction.
The existing Map primitive enforces `maxItems: 1` and collects reproduction evidence. Overflow or
indexed runtime errors stop publication. Failed behavioral checks and setup blockers are normal
evidence, so the checker can publish an honest report. No separate summarizer is needed.

Reproduction uses the full repository-defined local stack; UI changes require browser checks.
Agents don't repair reviewed code. The checker scrutinizes correctness and lean abstraction
boundaries, and checks all linked-issue requirements unless the PR/caller explicitly declares
partial scope. Missing issue text or required browser/stack verification remains an explicit gap.

The reproducer starts evidence with `Reproduction:` (at most 35 words: stack/browser, action,
observed result) and `Setup:` (at most 25 words: problems, workarounds and remaining blockers).
Resolved setup problems still count. The checker preserves both within its 150-word report budget;
a skipped reproduction gives its reason instead. These are prompt instructions, not enforced
length limits. The native header names captured revisions once and preserves other source provenance.

The [runtime example](../../docs/assets/pr-review/runtime.json) assigns all three agents
Codex/OpenAI `gpt-6.1-sol`, `xhigh`, in independent execution sessions. Models stay caller-authored.
`pr-review` supports comment delivery only; comparison preparation uses the admitted publisher.
`issue-review` keeps the basic proposal review, defaults to comment, and permits explicit `none`.
Further issue-review design is deferred. `software-change` defaults to PR delivery; its explicit
push/merge/none choices remain. Saved graphs retain their authored delivery.

## Acceptance evidence

CLI/provider work used `the-open-engine/zeroshot-matrix-demo-20260812-3ebd`.

| Case | Run | Observed result |
| --- | --- | --- |
| Issue 221 | `01a16a6e-a63b-7442-9cad-d7bd3b2c55f8` | [Posted review](https://github.com/the-open-engine/zeroshot-matrix-demo-20260812-3ebd/issues/221#issuecomment-6374021888), including a reproduced tenant-key collision. |
| Docs PR 225 | `01a16d52-346d-7931-b991-aa382f8a8258` | Classifier skipped reproduction; [53-word comment](https://github.com/the-open-engine/zeroshot-matrix-demo-20260812-3ebd/pull/225#issuecomment-6382776639), including provenance. |
| UI PR 224 | `01a16d9c-dcc0-7dc0-838b-1c747269e765` | Chromium against Node HTTP/SQLite reproduced two POSTs and persisted count 2 from one click; [113-word comment](https://github.com/the-open-engine/zeroshot-matrix-demo-20260812-3ebd/pull/224#issuecomment-6383576457), including provenance (99 report words). |

The latest UI run used head `d7c7b13e6abde5cd300c8758aaf3dd514485fe40` and linked issue 223.
PR/issue text and the fixture README contained no review hints; saved input had no caller
instructions or prior comments. All three PR agents used the checked-in `gpt-6.1-sol`/`xhigh` plan.
Browser checks covered reload/restart persistence and unrelated-state preservation. Cached
lockfile installs and prepared Chromium worked; the reproducer corrected an auxiliary capture
script typo and reported no remaining startup/credential blockers. Coverage was Chromium only;
concurrent clients and storage failures remain untested.

Receipt hashes matched GitHub comment bytes. Same-key retries returned the original runs with
one matching marker each, and tracked checkouts stayed clean. Evidence lives under
`/home/ec2-user/.cache/pr-review-acceptance-20261024/no-hints`, with docs-skip evidence in its parent;
issue evidence is in `/home/ec2-user/.cache/comment-delivery-acceptance-20261023/final-acceptance.json`.
These runs cover local CLI execution. Hosted/direct live runs and a live fork PR remain untested;
resolver and Git transport fixtures cover fork materialization.

Post-rebase checks passed native all-targets/UI compilation, 415 UI tests/build, 68 Python SDK
tests with lint/types/docs, and 67 tooling tests with ESLint/distribution checks. Final Rust workspace,
Clippy and documentation checks remain release gates. Platform checks include PostgreSQL-backed
resolver/CAS/migration tests and frontend integration using local native patches. PostgreSQL tests
cover SQL. The legacy Sense scan reaches its fixed repository limit; structural coverage is incomplete.

## Remaining rollout order

1. Finish rebased native checks and independent reviews; open a Conventional Commit PR to `main`
   with nonempty `## Summary`, then merge through required CI, including platform lanes.
2. Publish the canonical native `vX.Y.Z` release from merged `main`; verify immutable binaries,
   target image and SDK outputs through the existing release workflow.
3. Rebase Platform, replace external test patches with that release's immutable runtime/SDK pins,
   update Cargo lock/runtime lock/docs together, and rerun Platform checks against those artifacts.
4. Merge and deploy Platform through its existing pipeline, then verify issue/PR URL intake and
   publication in the deployed placement. Record hosted/direct/fork coverage when completed.
