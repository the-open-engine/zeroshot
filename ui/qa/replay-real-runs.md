# Real replay histories

Generated on 2026-09-17 against the verified test repository
`the-open-engine/zeroshot-matrix-demo-20260812-3ebd`, source revision
`8011f656b77ea69b0c1ea7c21d54edf636ea7dfe`.

## Processes

1. **Parallel launch packet.** Two writing agents independently create customer
   release notes and an operational checklist for the existing tenant quota
   service. They own separate files. After both finish, an independent verifier
   checks the packet and chooses ready or needs revision.
2. **Incident handoff review loop.** A writer prepares a deliberately incomplete
   first draft of a hypothetical support incident. Parallel reviewers check
   completeness and information safety. The completeness reviewer rejects the
   missing escalation and rollback sections; a writer uses the diagnostics to
   repair the file, and both reviewers inspect the revised version.

Both graphs were custom JSON documents validated through `zeroshot run
--validate-only` before submission. Workers write documents to disk; verifier
outputs contain only concise decisions and diagnostic evidence.

## Runs

| Run ID                                 | Title                                              | Result                                                                                                                                                                                      |
| -------------------------------------- | -------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `01a0af10-f2b3-7500-9b79-b9bd6782ce11` | Replay study: parallel launch packet               | Failed: the older system Codex CLI rejected the selected model. Both writers failed and the authored failure branch terminated the run.                                                     |
| `01a0af11-998d-7911-8c1b-00db4fafd854` | Replay study: parallel launch packet (current CLI) | Succeeded; 102 durable events. Two parallel writers completed, the verifier accepted, and repository checks passed (876 tests).                                                             |
| `01a0af12-7b9d-7a40-aae0-74394cb8c8a5` | Replay study: incident handoff review loop         | Succeeded; 126 durable events. Completeness rejected the first draft; the repair received both diagnostics; both reviewers accepted the second draft. Repository checks passed (876 tests). |

The first failure is real, not a fixture. `/opt/homebrew/bin/codex` was version
`0.141.0`; its provider response said `gpt-6-astra` requires a newer CLI. The other
runs use the current application's CLI at
`/Applications/ChatGPT.app/Contents/Resources/codex`, version `0.154.0-alpha.6.2`,
through a command-scoped `PATH`. No global installation or configuration changed.

Runtime selection is explicitly `codex` / `openai` / `gpt-6-astra`, low effort,
with `connections: {}` to use the existing local Codex login. Every executable
node has a 180-second deadline; the review loop has at most three iterations.

## Local evidence

- Durable histories: `~/.local/state/zeroshot/runs/<run-id>/runs.sqlite3`.
- Graphs, runtime, and inputs: `/tmp/zeroshot-replay-specs-20260917/`.
- Launch files: `/tmp/zeroshot-replay-matrix-20260917/replay/launch/`.
- Handoff file: `/tmp/zeroshot-replay-review-20260917/replay/review/incident-handoff.md`.

Work happened in a disposable clone and its isolated worktree. No product
checkout, saved profile, remote branch, pull request, or merge delivery was changed.

## Replay checks these histories support

- Simultaneous active nodes and out-of-order parallel completion.
- Agent commentary, file changes, command starts, and large command outputs.
- Provider failure, its bounded continuation, worker failure, and terminal failure.
- Different verifier decisions at the same review checkpoint.
- Feedback entering a later worker's recorded input.
- Multiple executions of one graph node across loop iterations.

The launch run has 91 `safe_log` records, including chunked TAP output. Navigation
should distinguish execution milestones from transcript volume. At the ledger
boundary, transcript text is `line` and the stream is `output`, `system`, or
`error`; logs carry a numeric execution identity. Node start and completion
records connect that identity to the graph node. Structural choices and groups
do not have their own durable events.

The review loop retains node-instance IDs across iterations: acceptance instance
2 executes as execution 2, then execution 5; code-review instance 3 executes as
execution 3, then execution 6. Both iterations report attempt 1. Replay must use
execution identity to separate these histories; attempt and node-instance alone
cannot identify the loop iteration.

## Simulated profile histories

`ui/public/history-examples.json` contains ten separate, explicitly simulated
examples. They are rendering fixtures, not native ledgers or claims of execution.
Their graph and runtime documents were copied unchanged from the saved profiles.
No example profile was edited, no credentials were copied, and no delivery ran.

| Profile                      | Expected inspection scenario                                                                                                   |
| ---------------------------- | ------------------------------------------------------------------------------------------------------------------------------ |
| `api-compatibility-release`  | Parallel plan writers; compatibility rejection; repair plan; second implementation and review pass.                            |
| `conference-planning-packet` | Parallel venue/agenda writers; budget rejection; second planning pass; packet assembly.                                        |
| `customer-onboarding-plan`   | Empty product map; parallel training and rollout; accepted final onboarding plan.                                              |
| `customer-support-drafting`  | Billing branch selected; unsupported refund promise rejected; revised reply accepted. Other support branches stay unreached.   |
| `database-migration-package` | Parallel compatibility/rollback reviewers disagree; worker adds a safe rollback cutoff; both accept.                           |
| `editorial-evidence-review`  | Three concurrent map items; separate artifact paths; nested parallel review repeated; approximately 13 KB evidence transcript. |
| `flaky-test-investigation`   | Three map items start; one times out; other two complete; authored failure terminates before report composition.               |
| `product-launch-content-kit` | Three parallel file writers, assembly, parallel reviews, repair plan, and a second complete revision.                          |
| `security-patch-merge`       | Simulated CI failure at delivery, repair, repeated reviews, and a simulated merge receipt. Nothing was pushed or merged.       |
| `service-incident-handoff`   | Parallel evidence writers, urgent classification, response plan, and accepted handoff.                                         |

There are 251 events across these examples. Every initial input and recorded node
input/output/diagnostic was checked against its stored graph schema; all cursors
are contiguous and execution references are unique. Graph/runtime snapshots were
compared with the source profiles and match exactly. These checks do not claim
native reducer conformance for the handcrafted event sequences.

Regenerate deterministically from the frozen definitions:

```bash
python3 ui/qa/generate-history-examples.py
```

The optional `--profiles` argument captures the ten named definitions from a
local profile file. It reads that file and writes only the example JSON. All
fixture runs have `example: true`, `example-` IDs, an explicit simulation notice,
and `source.profile` pointing to the original profile name.
