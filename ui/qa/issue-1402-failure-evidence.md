# Failed-run evidence and map paths

Verified on Linux with the candidate CLI built from this change on 2026-11-09.
The pinned matrix fixture revision was
`998c66fa78a9ee21a1b79466f7dc7d1c53cf47cf`; each run used a separate copy.
The prepared fixture was unchanged. No delivery or implementation job ran.

## Authentic provider runs

Both runs used the authored `codex` / `openai` / `gpt-6.1-sol` lane with low effort,
native local authentication, and Active Memory/postmortem disabled. Verifiers
read current fixture source and ran its 972 passing tests. Their review question
intentionally required a missing operator handoff document.

| Run                                    | Observed evidence                                                                                                                                                                                                                                                              |
| -------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `01a1c117-ab27-7e01-aa89-bd229ce40b6c` | `assessor` returned graph-defined `readiness: revise` twice. `handoff_refinement` exhausted after exactly two completed rounds; the authored failure was `needs_followup`. The latest diagnostic retained cursor `v2:45`.                                                      |
| `01a1c11a-d1f2-7331-8a5e-c77af48025f9` | Three `topic_audit` executions retained paths `[0]`, `[1]`, `[2]`. The quota item at `[1]` returned `revise` with a missing-checklist message at `v2:60`; the other items returned `ready`. Watch, terminal context and matching logs agreed on execution selectors and paths. |

A separate stub-Claude CLI run, `01a1c116-a0e6-74b0-8e8f-e9cdf32d557b`,
retried once per round. Four attempts remained two completed loop rounds.
Its synthetic ambient authentication marker was redacted from public logs and
terminal context with omitted connections. The adapter regression also checks
that same marker through declared credentials, including JSON escaping and
bounded UTF-8 diagnostic truncation.

## Browser and replay

Chromium inspected both real runs through the candidate's local UI. The loop
run displayed the latest recorded verifier result, actual diagnostic and named
two-round exhaustion. Its retained-log button selected the matching transcript.
Replaying to the start removed terminal evidence; returning to latest and opening
the run overview restored it. Reloading fetched the same durable evidence.
The map overview displayed `topic_audit[0]`, `topic_audit[1]`, and
`topic_audit[2]`; the second item's log button selected its own transcript.
The only browser console error was the pre-existing missing favicon request.

Local, ignored artifacts are under `.tmp/issue-1402/`: `ui-loop-evidence.png`,
`ui-loop-replay-start.png`, `ui-loop-replay-latest.png`, `ui-loop-reloaded.png`,
`ui-map-evidence.png`, `ui-map-cursor.png`, and the `e2e/` status, watch and log
captures. These are fresh candidate observations, not the issue's historical
baseline output. Nested paths, later acceptance, independent runtime causes,
terminal freeze and bounded/incomplete evidence also have focused automated
regressions beside their owning modules.

## Independent review and completion regressions

Independent behavior review subsequently exercised both required provider lanes
with high effort: OpenAI `gpt-6.1-sol` run
`01a1c144-1503-7db1-9d4e-74f418a08a59` and Anthropic `claude-opus-5-5` run
`01a1c146-65d0-7143-986a-14f74ccb034a`. Both separate pinned-fixture copies passed
all 972 tests. The OpenAI loop retained the latest verifier diagnostic and exactly
two completed rounds; the Anthropic map retained all three paths and the second
item's diagnostic. Browser checks covered keyboard log navigation, replay,
reload, a narrow viewport, and persistence after restarting the backend.
The review's report and captures are retained in `.tmp/verifier-execution2/`.

Two code-review regressions were then reproduced before their fixes: verifier
output suppressed a later cleanup timeout in terminal context, and Git-delivery
results were absent from verifier evidence. Both pass after the corrections.
The delivery regression drives actual local Git operations with a fake forge,
exhausts two rounds for both `ci_failed` and `repair_required`, and checks the
latest redacted result, execution identity, retained cursor, terminal snapshot
and history. The timeout regression checks that the verifier result and later
timeout both retain matching public log evidence. These checks use the existing
completion bridge and credential redactions without changing graph outcomes.
Fresh correction checks are recorded in `.tmp/issue-1402-review-fixes/`.
The final rebuilt candidate also read back all six independent review runs:
status, logs and retained UI history match the saved evidence exactly, and the
repository Python SDK parses their status and log projections. These final
checks replay retained evidence; they do not claim another provider invocation.

## Settled parallel joins

The latest code review reproduced lost exhausted-loop entries when parallel
branches selected authored failures and their join became unreachable. The new
supervisor regression failed before the correction, then passed for both `all`
and `any` joins. Reducer regressions also cover a settled nested race loser and
six discarded map paths; those paths neither appear as exhausted loops nor
consume the five-entry evidence bound. Plain and traced reduction agree.

Fresh stub-Claude run `01a1c1b7-b478-7c53-b2fd-1db779af0e9b` uses the rebuilt
candidate, SHA256
`19d41a59330bef3a5f61639820bcfe3727fe019d9e1018fc0befc027896a9fca`.
Both `left_rounds` and `right_rounds` report exactly two completed iterations.
The latest verifier results point to `v2:17` and `v2:14`; all four completion
messages remain in logs. Status, final watch, retained terminal history, and
Python SDK projections agree. History remains identical after restarting the
backend. This run makes no model calls.

Chromium visibly displays both loop entries and verifier messages. Replay at
the start removes terminal evidence and latest restores it. Keyboard activation
of the `v2:17` log button opens `left_assessor` and its matching transcript.
The ignored `.tmp/issue-1402-parallel-fix/` directory contains the commands,
captures, `ui-parallel-loops.png`, and `ui-parallel-log.png`.
The previously accepted real-provider checks are recorded separately in
`.tmp/verifier-execution5/report.md`; they precede this final reducer correction.

## Overflow after later convergence

The next review found that an overflow warning disappeared when later loop visits
converged and removed every retained loop entry. The supervisor regression now
checks the returned terminal result, stored snapshot, status and durable history
for both an overflowing case and a below-bound control. It failed before the
terminal projection correction and passes afterward.

Fresh stub run `01a1c1df-7457-7070-bd04-5ee3f6f155ac` retains `incomplete: true`
with an empty exhausted-loop collection and five latest `ready` verifier results.
Control run `01a1c1df-b6a2-7311-b19e-db9bf639c575` retains `initial_exhausted`
with one completed iteration and `incomplete: false`. Both use candidate SHA256
`54337a6b3b1fb597b3f5b43fbfa98a33e0fb366bc28025d2a25250d696eee25c`.
Status, final watch, logs, retained terminal events and SDK projections agree.
Chromium visibly retains the warning without loop entries; replay and a backend
restart preserve it. Captures are in `.tmp/issue-1402-overflow-fix/`, including
`ui-overflow-warning.png` and `ui-overflow-restarted.png`.
The accepted provider E2Es in `.tmp/verifier-execution8/report.md` precede this
last projection correction; these new stub runs make no model calls.
