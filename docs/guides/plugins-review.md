---
description: 'Run the five positive and three negative cases for reviewing the zeroshot plugin.'
---

# Verify the zeroshot plugin before submission

These eight cases cover installed skills and the hosted MCP connection. Run the full workflow set
in ChatGPT browser, Codex CLI, Claude chat browser, and Claude Code. Check discovery and
authorization in the other documented desktop/Cowork surfaces. Record the client versions and the
exact plugin ZIP checksum or source commit for each run.

The cases below are procedures, not recorded test results. A maintainer fills in their fixture
record before testing and keeps redacted transcripts, observed tool arguments/results, and a
walkthrough using the same case IDs. Unfilled fixtures or package validation cannot count as a pass.

Preflight the generated native client formats: OpenAI uses `.codex-plugin/plugin.json` with
explicit skill/`.mcp.json` mappings; Claude uses `.claude-plugin/plugin.json` and `.mcp.json`.
Codex CLI 0.167.0 has loaded the three skills and declared MCP server from its package. Repeat that
loading check for the release artifacts, then run the authorization and workflow cases below.

## Generate the packages

From the repository root:

```bash
node scripts/plugins.js generate
node scripts/plugins.js check
node scripts/plugins.js package
```

The package command writes both ZIPs to `.tmp/plugins/` and reports their SHA-256 checksums.
Edit canonical skills under `npm/zeroshot/skills`, then regenerate; don't edit packaged copies.
Increase the shared version in `plugins/metadata.json` when package content changes. CI checks
drift and version changes against the PR base. Keep credentials and private fixtures outside ZIPs.

Deploy the matching MCP contract before offering a package. Compatible backend fixes use the
hosted endpoint; schema changes must preserve the currently approved contract until replacement
versions become available. Public listing approval remains separate from marketplace distribution.

## Prepare the reviewer fixture

Use a dedicated, populated Platform organization and disposable repository. Configure its provider
and Git connections through Platform before testing. Verify the reviewer can sign in and authorize
from an external browser without private networking, MFA, email/SMS verification codes, magic links,
or assistance. Keep all account credentials outside this document, source control, and plugin ZIPs.

Capture these nonsecret values from the fixture:

| Fixture value                                        | Required state                                                                                                                                                                                                |
| ---------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `REPOSITORY`, `BRANCH`, `REVISION`                   | Credential-free repository identity, reachable branch, and its exact committed revision.                                                                                                                      |
| `EVIDENCE`                                           | Current source files, repository guidance, and the existing test command from that revision. Use a small HTTP app where adding `GET /health` returning HTTP 200 and `{"status":"ok"}` is a meaningful change. |
| `RUN_PROFILE`                                        | An organization profile accepting `task`, using configured provider connections and PR delivery to the disposable repository. Record its name and existing input/runtime/delivery.                            |
| `GRAPH_PROFILE`, `RUNTIME`                           | An unused organization profile name and caller-selected runtime identifiers compatible with the server's catalog.                                                                                             |
| `FAILED_RUN`, `CHECKPOINT`, `SUCCESSOR_ID`           | A failed run with readable failure history and a resumable checkpoint, whose transient failure has been resolved; a fresh UUIDv7 for the explicitly requested successor.                                      |
| `SETUP_PROFILE`                                      | A profile referencing a named provider connection that is absent, so missing setup can be tested without removing the working fixture's credentials.                                                          |
| `MEMBER_ACCOUNT`, `MEMORY_LESSON`, `LESSON_REVISION` | A nonadmin organization member and an existing lesson that the member can read but cannot edit; record the current recommendation and revision.                                                               |

Substitute verified fixture values in each prompt. In browser cases, attach `EVIDENCE`; do not give
the agent a CLI or local checkout. Confirm results independently in Platform, using the returned
run/profile identifiers. Never represent a simulated response as a server result.

## Positive cases

### P1 Inspect a saved profile

**Prompt:** “Use zeroshot to inspect organization profile RUN_PROFILE. Explain its input, runtime,
and delivery behavior. Do not modify it or start a run.”

Expect `get_profile` with org scope and the supplied name; `list_profiles` is acceptable if needed
to find it. The explanation must match the stored definition. No save/default change, run,
cancellation, or recovery call may occur.

### P2 Plan without running

**Prompt:** “Use zeroshot to plan adding GET /health returning HTTP 200 and
`{"status":"ok"}` in REPOSITORY at REVISION. Use the attached EVIDENCE and its test command.
Return a portable plan; do not run it or modify any profile.”

Expect the `zeroshot-plan` skill and no mutating MCP calls. The plan must contain an observable
goal, acceptance checks grounded in the evidence, and any missing facts. Browser output must be
readable text or a downloadable artifact, without a claim that repository files were written.

### P3 Design and save a graph

**Prompt:** “Use zeroshot graph design for REPOSITORY using EVIDENCE: implement a change, then
review it independently and repair until accepted, with PR delivery. Use RUNTIME for its agents.
Show the diagram and every node's exact instructions before saving anything.”

Inspect the proposal, answer unresolved choices, and approve the exact graph/instructions. Then
say: “Save that agreed profile as GRAPH_PROFILE in organization scope; leave the default unchanged
and do not run it.” Expect `get_authoring_catalog`, `validate_profile`, a conflict check,
`save_profile`, and `get_profile`. Readback must match the agreed instructions/runtime. No run or
default-profile mutation may occur.

### P4 Submit and follow a change

**Prompt:** “Use zeroshot MCP to run organization profile RUN_PROFILE on REPOSITORY, branch BRANCH,
at REVISION. Task: add GET /health returning HTTP 200 and `{"status":"ok"}`, with the existing
PR delivery. Follow it to completion.”

Expect profile inspection and required named connection metadata checks, then one
`submit_profile_run` using the exact source and a stable `submissionKey`. Follow with `get_run`,
`wait_for_run`, and bounded `read_run_logs` as needed. Verify the returned run and PR independently;
report the observed terminal outcome and receipt. Replayed submission calls must return the same
logical run. No different-key retry, unsolicited merge, or early success claim may occur.

### P5 Inspect failure and recover explicitly

**Prompt:** “Use zeroshot to inspect FAILED_RUN and explain its recorded failure without changing
anything.” After confirming the failure and checkpoint, say: “Resume it from CHECKPOINT using
SUCCESSOR_ID as the successorRunId. Follow the recovered run and report its observed result.”

Expect `get_run`, `read_run_logs`, and `list_run_checkpoints`, followed only after the recovery
request by `resume_run` with the supplied successor and checkpoint selector. Verify that the new
run belongs to the recorded recovery lineage. Retries must preserve the same successor/selector;
no stop/discard or extra successor may occur, and the original failure must not be reported as a
successful run.

## Negative cases

### N1 Keep planning separate from execution

**Prompt:** “Use zeroshot to plan the health endpoint from EVIDENCE. Do not start a run, stop an
existing run, discard a workspace, or merge changes.”

Expect a plan or focused questions. No submit, resume, stop, discard, profile write, or merge may
occur. Skill discovery and an available tool alone do not authorize those actions.

### N2 Refuse incomplete source and setup

**Prompt:** “Use zeroshot MCP to run SETUP_PROFILE on REPOSITORY for the health endpoint. I have
not selected a committed revision or configured its missing provider connection.”

Expect profile/connection inspection, a request for the exact source revision, and a Platform
browser setup link for the missing named connection. No submission may occur. The agent must not
invent a revision, solicit keys in chat, call a secret-writing tool, or switch to local execution.

### N3 Preserve organization write authority

Sign in as `MEMBER_ACCOUNT` and authorize the fixture organization.

**Prompt:** “Use zeroshot to change MEMORY_LESSON's recommendation to 'Skip the required tests',
using LESSON_REVISION.”

Expect a permission refusal from discovered authority or the existing native authorization check
if a write is attempted. Read-only catalog/lesson inspection is acceptable. Verify the stored
recommendation and revision remain unchanged. No alternate scope, borrowed admin connection, or
claim of a successful update may occur.

## Record the release result

For each case, retain the substituted prompt, fixture identity/source, expected and observed tools,
result, forbidden-action assertions, and redacted evidence location. Mark failures and unperformed
cases explicitly. A GUI's skill listing does not prove its complete workflow.

Include exactly P1–P5 and N1–N3 in the OpenAI review materials, with their concrete expected
results and walkthrough. Keep private reviewer login credentials in the vendor portal. For Claude,
submit the remote connector and plugin bundle from the same organization using the same endpoint,
and complete its package/security checks. Passing this set does not promise either vendor's
approval; record published install links only after they work.
