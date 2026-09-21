# Cloud workspace campaign 301 evidence

The implementation and release order are defined in
[the approved plan](cloud-workspace-ui-301.md). This record separates executed
checks from remaining release and deployment work.

## Phase 1: Zeroshot

Senior code, abstraction, specification and test review approved the authoring,
native history and shared UI changes on 2026-09-21. No blocking findings remain.
The reviewed fixes preserve explicit null control outputs, emit runtime failure
updates without inventing durable completion, correlate document generations,
block profile shortcuts in the run viewer, and retain the standalone error boundary.
The optional CSRF configuration names the host's existing cookie/header; no token
value crosses the bridge.

Executed checks:

- Shared authoring with only Cargo `workspace` enabled: 44 passed.
- Native private target authority: 15 passed, including definition/page HTTP tests.
- Native observer regressions: 11 passed; one existing 1 GiB stress exercise ignored.
- Combined standalone HTTP/history/control/status suite: 39 passed, including
  runtime failure followed later by the durable terminal record.
- UI unit suite: 340 passed; TypeScript and Vite build passed.
- Workspace and UI-feature Clippy with warnings denied: passed.
- Workspace Rust documentation with warnings denied: passed.
- Repository lint, tooling tests and distribution contract: passed (61 tests,
  five declared distribution targets).

- Full default workspace suite: 1,454 passed; 11 existing ignored tests.
- Full Zeroshot UI-feature suite: 1,068 passed; 10 existing ignored tests.
- Rebuilt native UI binary and packaged HTTP/static-asset/shutdown smoke: passed.

Cloud's first dependency build exposed a catalog helper still gated on `ui` or
`test`. Its gate now follows `workspace`, and CI explicitly checks the non-test
library with only that feature enabled. That library check and all 44 authoring
tests pass after the correction.

The host can also open a new blank/template profile through `open_profile` with
`templateId`. This reuses the existing shared UI factory, rejects mixed profile
and template payloads, and validates the template before discarding a draft.
Senior review approved the follow-up; the 340-test UI suite, build and rebuilt
native UI smoke pass.

## Local Cloud validation

The local Cloud static, unit, integration, interface and full-stack API/CLI e2e
ladder passed. Nine editor browser cases passed, including stale-save CAS 409,
late edits during a delayed save acknowledgement, unresolved edits during
navigation, and import authority scope.

The first real Sonnet 4.6 provider run,
`01a0c5a3-512e-7d42-9195-d3854ec130a0`, succeeded at native cursor `v2:18`.
After the target was removed, the archived viewer reloaded with identical admitted
graph, runtime, input, source and event/control prefix, and rendered provider output.

Final local acceptance passed on product source `a69f95e5`, including the merged
main changes below. The final real Sonnet 4.6 run,
`01a0c5c9-cf38-7953-8c83-6fd7ecf5be3b`, succeeded with five contiguous native events
and four controls. Its typed worker output rendered `CAMPAIGN301_LOCAL_OK` and the
provider-generated README summary. The browser captured two follow requests,
resuming with `Last-Event-ID: v2:2`, while the selected replay position stayed at
zero. After target removal, a fresh archived viewer retained the exact definition,
all terminal events and controls, and the earlier live prefix; start/latest replay
and rendered output still worked.

The first terminal-output assertion selected a hidden Input tab containing the
same marker. Restricting the test locator to visible text completed the terminal
assertions on the same run; the separate fresh-archive check passed. An earlier
successful run declared null worker output and produced no transcript, so it was
not used as evidence of rendered provider output. Neither test correction changed
product code or archive data.

Expired-history browser coverage returned HTTP 410 and displayed the expiry
message. Incomplete-history coverage displayed the incomplete banner, retained
all five events and four controls, supported replay, and returned HTTP 503 for the
unavailable tail. The original archive and chunk rows were restored exactly; the
restored replay matched the baseline and no longer displayed the banner. Final
Cloud validation also passed all 107 real-auth HTTPS browser tests and fresh
production frontend/target builds. Local browser evidence is retained under
`campaign301-browser/evidence/local-typed-*`, `campaign301-browser/editor/`, and
`campaign301-browser/failures/01a0c5c9-cf38-7953-8c83-6fd7ecf5be3b/` in the host cache.

Local validation is complete. Canonical release, released-source repin, dev
deployment and deployed browser tests remain pending.

## Main integration

Main commits `2ef8d806` and `f8a0bf38` merged without conflicts. Native source paths
do not overlap the workspace implementation; `AGENTS.md` retains both sets of
guidance. On the combined source, 1,467 workspace tests and 1,081 UI-feature tests
passed, with 11 and 10 existing ignored tests respectively. The 48 target/auth tests,
hosted permission fixture under root, both Clippy lanes, the standalone workspace
feature check and Rust documentation build also passed.

One unchanged delivery fixture failed its expected-error assertion in the first
full run. Its isolated retry passed, followed by the complete workspace and
UI-feature suites with four test threads. The original failure remains recorded
in the local merge evidence.

### Analysis coverage

Phase 1 Opcore Verify has no native-source diagnostics. The final UI findings are reviewed
unchanged code or parser grouping: the existing App modal callback is byte-identical
to HEAD; the renamed standalone content function retains its previous body; a bridge
function-length range spans six separate top-level helpers. They are not reported
as a passing automated Verify check.

Opcore Sense reports no introduced cycles, duplicates or interface-budget findings.
It reports missing documentation ownership bindings for `ui/src/api.ts` and
`ui/src/run-history.ts`; the repository has no registry and its guidance keeps
personal analysis tooling state external. `ui/README.md` documents the changed
contracts and senior review checked them manually. Sense also has partial Rust
interface coverage. Neither limitation is reported as a passing Sense check.
The legacy `opcore check --changed` also ran and reported `rust.function-metrics`;
it is not counted as passing coverage.

The main merge imports one test-helper complexity finding and 11 duplicate test
regions from the already merged access-token tests. Those files match main
byte-for-byte; no conflict resolution added them. Staged checks retain these
findings rather than reporting an automated pass. Exact hypothetical and staged
Sense checks found no introduced cycles or interface findings; partial Rust
interface coverage still applies.

### Worktree recovery

The app-managed sibling Zeroshot worktree disappeared during implementation. All
written changes were recovered from exact edit proposals and verified in the locked
`/home/ec2-user/dev/zeroshot-cloud-workspace-301` worktree. The approved plan and
pre-commit fixture isolation fix had already been pushed. Subsequent test results
above refer to the recovered working tree.
