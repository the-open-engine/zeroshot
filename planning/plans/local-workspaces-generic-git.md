# Local workspaces and generic Git transport campaign

Status: implemented; local acceptance is complete. The campaign branch carries validation results without a campaign PR.

Baseline: `origin/main` at `bb84bf736709114605fef1ab80400fffc65569d9`.
Scope comes from the user's request for local runs from ordinary folders, optional working-folder
selection, and generic Git transport including push. Deliver these together through one campaign
branch pushed to origin, without a campaign PR. PR and merge operations against the disposable
matrix repository are authorized acceptance tests. Use one integrated acceptance gate. Use the existing worker, verifier, and delivery workflows.

## Result

Workspace preparation and repository resolution become independent. A local graph without Git
delivery runs in an ordinary folder, including a Git checkout without a remote. Checkout and
push use shared Git transport. GitHub remains the maintained forge integration for PRs, feedback,
required checks, and merges.

| Placement and graph                               | Admission requirement                                                               |
| ------------------------------------------------- | ----------------------------------------------------------------------------------- |
| Local without delivery                            | Existing folder; repository source is optional.                                     |
| Local with push                                   | Git worktree, committed candidate basis, target branch, and selected Git remote.    |
| Local with PR or merge                            | Git worktree and a GitHub repository supporting the requested forge operations.     |
| Docker or Platform without delivery, or with push | Git repository and exact source revision reachable through supported Git transport. |
| Docker or Platform with PR or merge               | Git repository plus the maintained GitHub forge integration.                        |

Existing target delivery policy remains authoritative; accepting a Git source doesn't waive a
target's requirement for a delivery node.

## Shared design

Keep one prepared workspace whose path stays host-private and whose filesystem identity is stable.
Local directories remain user-owned. Repository provenance is optional in local submissions,
admitted state, ledgers, and public projections.
Delivery configuration is one optional object; don't construct dummy repository identities or
an unused delivery adapter for graphs containing only agents. Preserve per-node runtime lanes.

A resolved Git source carries a credential-free remote locator, selected branch, and exact commit.
It may also carry a canonical forge identity. Shared Git code doesn't validate every repository as
GitHub `owner/name`, infer a forge from a model/provider, or call forge APIs to confirm Git refs.
GitHub owns its repository grammar and review policy. Keep source selection separate from the
delivery branch and retained recovery lineage.

Use one shared Git transport implementation for remote-ref reads, exact fetch/checkout, and push.
Extend the existing supervised process and Git command boundaries rather than creating another
subprocess framework. Keep local staging, commits, ancestry reconciliation, and delivery receipts
under the existing trusted delivery controller. GitHub operations delegate their Git effects to
the same transport.

Keep the existing source-provider contract and registry as the single provider extension point.
Evolve the unused contract against production Git/GitHub operations: materialization needs a scoped
Git capability; requests contain intended operations and preconditions, while receipts contain
confirmed results such as newly allocated review IDs and the resulting merged revision. A Git-only
provider supports repository access and push; the maintained GitHub provider adds forge capabilities.
Contributor adapters may reuse transport and implement only the capabilities they advertise.

Keep the existing push, PR, and merge nodes as the workflow interface. Adding a GitLab or Forgejo
integration should require registering an adapter that supplies repository identification,
credential requirements, and forge operations through the shared contract. The engine selects it
from the resolved source and checks its advertised capabilities during preflight. Repository and
review IDs remain opaque to the workflow. Adding a provider must not require another node family,
provider-specific templates, or changes to the common delivery loop. Introduce a new node or
contract version only for a new operation or an incompatible input/output contract.

Keep contracts independent of process/network implementation code. Put maintained implementations
under `source_code/adapters/core/`, and contributor implementations under
`source_code/adapters/contributed/`. Document registration and the shared contract tests; don't
create speculative provider stubs or a runtime plugin loader. Shared Git transport stays in the
engine layer so adapters don't duplicate it.

## Working folder and local admission

Add `zeroshot run --cwd DIR`, defaulting to the invoking directory. Resolve relative paths in the
invoking process without a process-wide `chdir`. Preserve current Git-root normalization when the
selected folder is in a worktree; otherwise use the selected folder. Input, graph, runtime, and
environment file arguments remain relative to the invoking directory. For target runs, `--cwd`
selects the client checkout inspected for source selection; the target receives the exact remote
revision, not the local folder or uncommitted files.

Determine requirements from validated graph/runtime bindings, including nested and conditional
delivery nodes. A plain-folder run requires neither Git nor GitHub authentication. Detached and
unborn repositories are valid without delivery. Push requires the Git context it uses, while PR
and merge require a supported forge. Apply local folder and delivery checks during run preflight,
including local `run --validate-only`; reusable profile validation stays source-neutral.

Select a compatible attached upstream first, otherwise one unambiguous eligible remote. Don't
require a remote literally named `origin`. Retain explicit source overrides and GitHub
`--repository OWNER/NAME` shorthand; generic source selection also accepts a credential-free clone
URL. Fail before controller launch when a required remote or delivery base is ambiguous.

For local runs, the resolved workspace and local state root must be disjoint: neither equals or
contains the other. Check canonical paths before creating run storage or launching a controller.
Resolve symlink aliases and a not-yet-created state path consistently. Report both paths and direct
the caller to another workspace or an external `ZEROSHOT_STATE_DIR`. Keep the checkpoint overlap
guard as a second check. Add no snapshot exclusion rules or automatic state relocation.

Include workspace identity in local submission reconciliation so the same submission key cannot
silently return a run in another folder. Aliases of the same workspace share its existing lease;
different disjoint folders can run independently. Resume uses the stored workspace and original
delivery lineage. ACP shares workspace preparation without acquiring a repository requirement.
Support plain-folder and unborn-Git checkpoints while preserving existing linked-worktree and
shared-ref restore restrictions. Zeroshot never initializes Git merely to admit an ordinary folder.

## Git transport and credentials

Transport supports HTTPS and SSH remote forms, including Git's SSH shorthand. Preserve authored
remote paths and SSH usernames. Contained source admission rejects local filesystem remotes and
executable transport schemes; hermetic tests may use the existing host-owned fixture override.
GitHub host URLs and API credentials stay within the GitHub adapter.

Local Git reuses native Git configuration and authentication from the captured invoking context,
including credential helpers and SSH agent settings. Contained Git uses explicit host-owned or
named connection authority, never ambient caller credentials. Reuse the existing connection
resolver and source-connection mechanism for private HTTPS/SSH access and dynamic refresh. SSH
host verification remains explicit. Missing access reports a transport error; it doesn't fall
back to a different source or request interactive authentication.

Credential values stay out of source records, submission digests, checkpoints, observations,
diagnostics, and agent environments unless independently authorized for that agent. Any temporary
credential material belongs to private runtime storage and its existing cleanup authority.
GitHub tokens and installation connections retain their existing behavior; they don't become
mandatory credentials for generic Git or a public clone.

Push publishes the captured candidate SHA to the managed branch and confirms that exact remote ref
through Git. A lost push response triggers bounded remote observation before retry or repair.
Retain identity/ancestry checks, cancellation, deadline handling, credential refresh, and the root
attempt's branch identity across resume. Preserve published ancestry and local work when external
updates require repair. PR/merge continue using the current GitHub policy, feedback, and authoritative
merged-result checks; generic transport does not grant forge merge authority.

## Implementation order

1. **Shared contracts and transport.** Separate workspace, Git source, and optional forge identity.
   Make delivery setup optional. Introduce the shared transport and route current GitHub checkout
   and push through it. Implement provider registration against this boundary, adapting existing
   request/receipt types only where production requires it. Preserve current GitHub behavior and
   the effective runtime lanes.
2. **Local folder execution.** Add `--cwd`, graph-derived preflight, optional source projections,
   early state-overlap rejection, folder-sensitive idempotency, and lifecycle support through
   recovery/checkpoints and ACP. Update Python request/projection surfaces so one client can select
   different working folders for individual submissions.
3. **Generic source and push admission.** Broaden named-target source resolution, contained
   checkout/authentication, and the push delivery path to non-GitHub remotes. Generated push
   bindings must not acquire an unconditional GitHub connection requirement. Require GitHub only
   for PR/merge and retain all existing delivery/recovery checks. Finish documentation and the
   integrated regression evidence before pushing the campaign branch.

The campaign owns protocol, CLI, runtime, and SDK changes together. Keep existing ledger records
readable. Preserve existing GitHub submission bytes and digests when the new source capability
isn't used. Advertise new generic source/credential forms through a separate versioned discovery
extension, following the node-runtime-lanes precedent, and refuse incompatible targets before
sending a new request body. Don't weaken strict hosted route objects or add endpoint aliases.
Regenerate protocol artifacts and CLI reference docs with the repository tools.

## Acceptance gate

Use deterministic fixture harnesses and Git repositories for the executable acceptance cases.

| Evidence                                                                         | Required result                                                                                                             |
| -------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------- |
| Plain-folder local CLI run, with Git and `gh` unavailable                        | Admits, writes the expected artifact, reports source absence, and completes.                                                |
| Remoteless, non-GitHub, detached, and unborn local repositories without delivery | Admit without remote/branch requirements; committed and unborn checkpoint cases restore correctly.                          |
| Two `--cwd` submissions from one launcher                                        | Use different resolved workspaces, maintain independent leases, and never deduplicate across folders.                       |
| Local preflight failure cases                                                    | Missing delivery Git context, unsupported PR/merge forge, ambiguous remote, and workspace/state overlap fail before launch. |
| Generic remote checkout through direct and hosted composition                    | Runs from the exact admitted commit, including nested repository paths and explicit private transport credentials.          |
| Generic push with `gh` absent or a failing spy                                   | Publishes and confirms the exact managed-branch SHA with zero GitHub API calls.                                             |
| Push accepted but response lost, and concurrent remote-head changes              | Observes confirmed success or preserves candidate work and returns existing bounded retry/repair outcomes.                  |
| Resume, checkpoints, and legacy stored source records                            | Retain workspace and delivery lineage, decode existing records, and resolve only authorized fresh credentials.              |
| GitHub PR/merge regression                                                       | Retain exact-head feedback handling, required checks, policy enforcement, cancellation, and authoritative merge receipts.   |
| Fake contributed forge adapter with non-GitHub repository and review IDs         | Existing PR/merge nodes execute through the registered adapter without provider-specific workflow changes.                  |
| Older target discovery                                                           | Existing GitHub submissions retain compatibility; generic source/credential submissions fail before transmission.           |
| Logs, public records, checkpoint storage, and provider children                  | Contain no transport credential values introduced by this campaign.                                                         |

Run focused module and CLI tests first, then the affected native, Python, protocol, documentation,
and UI/history lanes. Follow `AGENTS.md` for cross-platform checks and update it for the new ownership
and admission rules. Run Opcore Verify after coherent source edits and Sense for interface/module
changes. Deterministic fixtures prove failure cases; authorized Codex and Claude matrix runs exercise the complete provider and delivery paths.

## Scope boundary

This campaign ships local-folder execution, generic Git checkout/push, and the usable provider
boundary. GitLab and Forgejo PR/check/merge implementations, additional GitHub Enterprise feature
coverage, runtime plugin loading, non-Git contained sources, and Platform product deployments are
separate work. Existing per-node model/harness selection and authored runtime configuration remain
caller-owned. Make no changes to `CLAUDE.md`.

## Acceptance evidence

Fresh workspace, source-boundary, transport, and integrated reviewers inspected the campaign.
The dedicated chaos tester exercised CLI subprocesses, ACP sessions, workspace checkpoints,
authenticated HTTP recovery, lost push responses, and concurrent branch changes. Findings were
fixed and rechecked, including ambient Git selectors, overlapping leases, private Git diagnostics,
credential-helper suppression, contributed remote discovery, ambiguous provider matches, and explicit
release of fork-inherited lease locks.

The matrix repository is `the-open-engine/zeroshot-matrix-demo-20260812-3ebd`.
Six provider runs completed successfully:

| Case                              | Independent confirmation                                                                                              |
| --------------------------------- | --------------------------------------------------------------------------------------------------------------------- |
| Codex in a plain folder           | 898 matrix tests; no Git initialization                                                                               |
| Claude in another plain folder    | 897 matrix tests; independent workspace                                                                               |
| Native SSH push                   | Exact remote SHA; 908 matrix tests; GitHub CLI spy untouched                                                          |
| GitHub PR and merge               | PR 212 merged as `f50c062c1cbbb1a2dc41d166b886e7e50e349acb`; required check succeeded; 900 tests on the merged commit |
| Contained HTTPS checkout and push | Exact remote SHA; 932 matrix tests                                                                                    |
| Contained SSH checkout and push   | Exact remote SHA; 937 matrix tests                                                                                    |

The contained runs used Bedrock-backed Claude with separate private source connections. Their
public observations contained neither HTTPS credentials nor SSH private-key material. The SSH run
also verified OpenSSH under the pinned writer identity without an operating-system account.
Target-image smoke covers that NSS mapping, native harnesses, and the advertised toolchains.

The Rust library suite passed 1,288 tests; seven opt-in tests stay separate. The opt-in matrix HTTP
recovery test passed, with five independent 865-test validations. Provider contribution tests drive
opaque repository/review IDs through existing PR and merge nodes and verify both CLI discovery
paths without provider-specific selection logic. UI tests passed 381 cases and Python passed 48.
The Rust UI-feature library passed 1,426 tests. A second chaos pass reproduced the inherited-lock
failure on the old code and passed 125 injected invocations, covering 200 lease tests, after the fix.
Root process/filesystem boundary checks passed all 27 cases in an isolated container with required
user namespaces. Protocol artifacts and CLI references come from their Rust generators.

Opcore Verify covered every changed file. Sense reported no introduced findings but partial
language/interface coverage; compiler checks, contract tests, fresh reviewers, and provider runs
supply the remaining evidence. Cross-platform CI runs on the pushed branch through workflow dispatch.
