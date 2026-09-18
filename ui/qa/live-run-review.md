# Live run verification

Verified on 2026-09-18 against the test-matrix repository
`the-open-engine/zeroshot-matrix-demo-20260812-3ebd`, revision
`8011f656b77ea69b0c1ea7c21d54edf636ea7dfe`, in the isolated worktree
`/tmp/zeroshot-live-matrix-20260918` on `codex/live-quota-batch`.

## Process and real provider runs

A code writer adds `FixedWindowRateLimiter.inspectMany` and regression tests for the
support console's read-only batch quota inspection. A parallel documentation writer
owns a separate runbook. After both settle, an independent verifier checks the API,
runbook and repository tests. Two focused follow-up audits inspect the result and
difficult caller inputs. Writing agents return files; verifiers return small verdicts.

Runtime: Codex / OpenAI / `gpt-6-astra`, low effort, existing local authentication.
The command-scoped CLI is `/Applications/ChatGPT.app/Contents/Resources/codex`,
version `0.155.0-alpha.9`. No global provider configuration changed.

| Run                                                                | Result                                                                                                                                                       | Retained cursor |
| ------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------ | --------------- |
| `01a0b363-c4a4-75f1-aca3-07964a614ca5` — batch quota inspection    | Both parallel writers completed. The verifier hit its authored 300-second timeout after printing the full test suite; the graph failed with `review_failed`. | `v2:122`        |
| `01a0b394-ae3f-7000-913f-cf70ef079fe8` — acceptance audit          | Accepted. Bounded command output; all 884 repository tests passed.                                                                                           | `v2:20`         |
| `01a0b395-bf27-7142-8298-4d96ced03d1c` — input compatibility audit | Accepted after inline probes and the repository test gate.                                                                                                   | `v2:22`         |

These are native provider histories, not simulated events. The first failure remains
visible. The final two audits do not rewrite it. Graphs, inputs, runtime and validation
output are retained in `/tmp/zeroshot-live-specs-20260918/`; native ledgers remain in
the standard local run store. No remote branch, PR or delivery was created.

## Browser observations

- The new running job appeared automatically in the run list. Its two parallel agents
  showed Running, and the selected writer's transcript grew without refreshing.
- Reading earlier transcript output changed Live to Go live. The transcript text
  stayed byte-for-byte unchanged while the durable run advanced, including through
  terminal failure. Going to latest revealed the retained final result.
- The acceptance audit streamed through completion and settled into ordinary replay.
- During the compatibility audit, the UI server was deliberately terminated while
  the run controller continued. The view showed Reconnecting and retained its graph.
  After the server restarted, the browser resumed automatically and received the
  remaining transcript and terminal success without reloading or a cursor-gap error.
- A fresh browser reviewer independently inspected the failure and both successful
  audits. Structured output, timeout diagnostics, node replay from global start and
  run switching worked. Completed runs had no stuck Live, Reconnecting or refresh
  controls. Desktop and 575 × 900 layouts had no page overflow; viewport restored.

## Regression coverage

- Native stream tests cover initial empty history, live append, terminal draining
  across bounded pages, resume cursors, large execution identities, invalid cursor
  headers, safe history-gap failure and SSE HTTP framing.
- Browser transport tests cover native reconnect, permanent connection closure,
  terminal delivery, malformed pages, callback validation failure and disposal.
- Projection tests pin a log-only cursor while new logs and completion arrive;
  future transcripts and outcomes remain hidden. Existing concurrent-execution and
  replay coverage remains in place.
- Fresh code review caught final transcript tailing, user cursor changes during
  initial paging, inaccurate Live labels before connection, and recovery actions.
  All were corrected and re-reviewed.

Final validation: 285 UI tests and all 62 profile-UI Rust tests passed, as did the
frontend and UI-enabled Rust builds, Rust formatting and whitespace checks.
Clippy remains blocked by two pre-existing `nonminimal_bool` findings in the
Claude and Codex command adapters. The test-matrix gate passed all 884 tests.

## Transcript and control-history follow-up

The earlier scroll-to-pause behavior above was replaced on 2026-09-18. Scrolling within a
transcript now preserves Live. Only a transcript at its bottom follows incoming output.
Reading an earlier entry also pins the inspected execution locally, so activity from a
parallel worker or a new loop visit cannot replace it. Reaching the bottom or explicit
navigation releases that pin; the graph cursor continues following throughout.

The zero-execution choice/finish bug came from treating every node as a dispatched worker.
The native ledger records worker starts and completions; structural operations happen inside
the reducer. An optional canonical reducer trace now reconstructs control visits at each
durable lifecycle prefix. These updates travel beside the original events, with their source
cursor and distinct map/loop visit identity. No logs or execution IDs are invented.

Read-only checks against retained real histories:

- `01a0b3a1-ef0e-7bd2-b91d-26358f1db042`: `review_result` selects `done` at `v2:62`.
  Both show one visit, and the finish result is Succeeded. Neither appears before that cursor.
- `01a0af12-7b9d-7a40-aae0-74394cb8c8a5`: two review-result visits, first `review_repair`
  at `v2:53`, then `done` at `v2:125`. Keyboard selection and replay from start show the
  respective paths without leaking the later decision.
- `01a0b363-c4a4-75f1-aca3-07964a614ca5`: the failed review selects `review_failed`
  at `v2:121`. Provider output remains available.

Isolated headless Chromium exercised the actual UI at desktop and 575 × 900 sizes, with no
page errors or horizontal overflow. The desktop computer-use surface was unavailable because
the Mac was locked. A separate mocked SSE connection tested the full run view with two
parallel workers: after more than 60 entries, appending own and peer output preserved the
first rendered entry, scroll position, selected transcript and Live state. Bottom scrolling,
pagination, filtering, expansion, backward seeks and Go live all passed. No native ledgers
were modified and no additional provider run was started.

Evidence is under `~/.codex/artifacts/zeroshot/`: `control-history-ui-qa.json`,
`transcript-sticky-scroll-qa.json`, `live-transcript-parallel-integration-qa.json` and
their adjacent screenshots. Temporary test pages and servers were removed.

Follow-up validation: 296 UI tests, 42 reducer tests and 67 Rust profile-UI tests passed.
Frontend/Rust builds, formatting and whitespace checks passed. A fresh reviewer checked
both the UI merge/selection logic and reducer projection. Clippy still reports only the
two pre-existing boolean-expression findings in the Claude/Codex command adapters.
