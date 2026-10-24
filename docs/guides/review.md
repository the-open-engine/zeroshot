# Review a pull request

The `pr-review` template checks a PR's implementation and posts a concise Markdown comment.
PR review uses comment delivery so comparison preparation always has an admitted publisher.
The separate `issue-review` template retains a basic issue-proposal review; its review workflow
will be designed separately.

## What the graph checks

The classifier inspects the captured diff and linked requirements. It requests one reproduction
for a fix, feature, UI change, or material correctness, security, persistence, or integration risk.
It skips reproduction only for clearly small, uncritical changes such as documentation or a
mechanical refactor. Missing tools or services cannot justify skipping a required check.

When requested, the reproducer exercises the changed workflow through the full local stack.
UI changes require browser checks. It records command and browser results, including failed checks
and environment blockers, without repairing reviewed code. A missing prerequisite remains an
explicit verification gap.

The code checker then inspects the diff and its callers for correctness, lean implementation,
and clean abstraction boundaries. It checks full coverage of linked issue acceptance criteria
unless the PR or caller explicitly declares partial scope, and incorporates the reproduction
evidence into the final comment. The comment has a 150-word instruction budget: actionable
findings, a brief `Reproduction:` sentence naming the stack/browser, user action and observed
result, and a `Setup:` sentence reporting startup problems and workarounds, including resolved
ones. A skipped reproduction gives its reason. Detailed evidence stays in run history or artifacts.
The native header records the captured revisions once.

The native publisher posts the report after these agents settle. A successful run confirms
completion and publication; findings or blocked tests can still appear in its report.

## Prepare the runtime and credentials

The supplied [runtime plan](../assets/pr-review/runtime.json) assigns all three agents
`gpt-6.1-sol` at `xhigh` on Codex/OpenAI with independent execution sessions. Save it as
`runtime.json` and pass it through `--runtime-config`. The publisher is native and has no model.
Model choices remain in the caller's runtime plan rather than the graph.

Comment delivery uses the `github` connection's `GH_TOKEN`, as other GitHub delivery does.
Provide that field through a named connection or the invoking environment. For a local run using
a stored GitHub CLI login, resolve `GH_TOKEN` from `gh auth token` before submission. Publication
credentials belong to the native node and are not declared on the review agents.

Local metadata resolution accepts `GH_TOKEN`, `GITHUB_TOKEN`, or a native GitHub CLI login.
Named targets resolve metadata through their task-resolution route. Hosted targets use host-owned
credentials; direct targets require `GH_TOKEN` or `GITHUB_TOKEN` in the invoking environment.
Targets must advertise task resolution and comment delivery; an older target refuses before submission.

## Run the review

Select a clean checkout already at the PR head. Zeroshot refuses a mismatch instead of switching
or resetting the worktree.

```console
zeroshot run --title "Review PR 123" \
  --template pr-review \
  --task-from https://github.com/OWNER/REPOSITORY/pull/123 \
  --runtime-config runtime.json \
  --cwd /path/to/pr-checkout \
  --submission-key review-pr-123
```

For hosted execution, add `--target platform` and select a repository environment that prepares the
required stack and browser tools. Source comes from URL resolution, so a client checkout is
unnecessary. A saved PR review profile already contains its runtime and publisher; use
`--profile org:pr-review` without template or delivery overrides.

`--validate-only` prepares and validates without executing agents. Ordinary `--input FILE`
passes JSON unchanged and is mutually exclusive with `--task-from`.

## Input and result

Fetching happens before the graph starts. PR intake captures `{url, context}` with title/body,
complete `context.reviewScope` base/head identities, and required `context.linkedIssues`.
Linked issue entries contain `{url, title, body}` from GitHub's manual or keyword-based closing
relationships. Plain mentions do not create that relationship; the checker must report referenced
requirements as unverified if their issue text is absent. An empty array means the selected GitHub
identity exposed no linked closing issues. Incomplete or erroring metadata refuses preparation;
intake never silently truncates issues. The bounded snapshot supports up to 100 linked issues.
See [GitHub's linking rules](https://docs.github.com/en/issues/tracking-your-work-with-issues/using-issues/linking-a-pull-request-to-an-issue).

There is no mandatory `task` field. Optional caller `instructions` can narrow review focus or
declare partial implementation. PR comparison uses the admitted head against its merge base with
the captured base revision, including fork heads; publication refuses a changed review scope.
The graph accepts PR context; select `issue-review` with an issue URL for the separate basic path.

The result contains a `receipt` with the comment permalink, identity, and body hash. Comment
publication never commits or pushes the review workspace. A custom graph can supply `{url, body}`
directly without an agent or local repository.

Retry uncertain submission with the same explicit submission key and authored configuration.
The CLI retains prepared input/source, so retries do not silently read newer discussion text.
Resume uses admitted input. One immutable comment belongs to each delivery lineage: matching
retries reuse it, while changed report content refuses with its URL. An uncertain POST reconciles
remote evidence and fails explicitly if unconfirmed; it never authorizes a blind repost.
