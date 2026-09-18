# Standalone integration verification

## Processes to exercise

1. **Retry-header support.** A coding agent adds a safe HTTP `Retry-After` parser
   to the matrix service, with regression tests and operator documentation. The
   standard software-change graph reviews the implementation and repairs it when
   needed. Observe live transcripts, review decisions, and final replay while the
   standalone UI process is stopped and restarted.
2. **Parallel deployment packet.** Two writing agents create separate deployment
   and rollback plans for the same service. The profile is built and saved entirely
   through the UI. Execute that saved profile with a real provider and inspect both
   branches and their file outputs.
3. **Container-hosted maintenance.** Run a realistic matrix-service task through a
   direct Docker target. Inspect it through the target's embedded UI, retain its
   profile and completed history across container replacement, and verify that an
   interrupted active run settles honestly after restart.

All work uses isolated matrix checkouts. No delivery, remote changes, or product
repository modifications are requested from providers. Results below are recorded
after observation; planned checks are not evidence of success.

## Real-provider results — 2026-09-18

Source: `the-open-engine/zeroshot-matrix-demo-20260812-3ebd`, exact revision
`8011f656b77ea69b0c1ea7c21d54edf636ea7dfe`. Local runs used an isolated worktree and
Codex/OpenAI `gpt-6-astra`; Docker runs used fresh target checkouts and the image's
pinned Codex/OpenAI `gpt-5.4`. No code was pushed or delivered.

| Host | Process | Run ID | Recorded result |
| --- | --- | --- | --- |
| Local | Retry-After implementation and two reviews | `01a0b402-3637-7200-8587-5c097ea4022b` | Succeeded, `v2:62` |
| Local | UI-built parallel deployment/rollback writers | `01a0b405-aee4-7752-a3f1-c2cb726bfa6d` | Succeeded, `v2:112` |
| Local | Release packet summary from generated files | `01a0b419-3ba0-7bb3-8e0a-6c9befce4328` | Succeeded, `v2:25` |
| Docker | Retry-After implementation and two reviews | `01a0b40c-33c5-7111-842f-ea4aa2f101f7` | Succeeded, `v2:125` |
| Docker | Release-readiness audit | `01a0b40f-e75a-7511-a7a3-2c95d2527a21` | Succeeded, `v2:94` |
| Docker | Coding run stopped during its first test command | `01a0b411-e4d3-7f41-a527-9fc88c11f93e` | Reconciled as `runtime_lost`, `v2:44` |
| Docker | UI-built release-checklist profile | `01a0b420-c961-7750-bcbe-5c46fcfd2971` | Succeeded, `v2:76`; returned `checklistPath` |

The final Docker run used the saved editor graph and runtime unchanged except for
declaring its named provider credential. Its writing Agent received `task` and
`releaseVersion`, ran the real repository tests, and returned the checklist file
path. Browser inspection confirmed typed Input, compact Output before Transcript,
71 transcript entries, and one successful completion visit. The local modified
matrix also passed an independent `npm test` run: 886 tests.

## Browser and lifecycle checks

Computer-use testing exercised the rendered UI and real keyboard/pointer input.

- Built/saved profiles, reloaded them, rejected stale cross-tab saves, recovered
  through Save as, and retried a failed offline save without losing the draft.
- Fixed incomplete numeric edits (`1e`): exact text survives node changes and
  inspector close/reopen, blocks saving even while hidden, and never commits its
  valid prefix. Escape restores the previous value; `1e3` commits as `1000` on
  blur. Explicit profile discard clears pending edits. Unsupported worker retry
  controls no longer invite configurations that admission rejects.
- Observed simultaneous writing agents and reviewers, replayed from the beginning,
  returned to live, and checked separate control visits and worker executions.
- Stopped the local UI while its provider was active. The UI exited cleanly in
  0.003 seconds; the run continued. Restart reconnected the browser without
  resetting its rewound playhead.
- Stopped Docker with an active transcript stream: clean exit in 0.158 seconds.
  Recreating the container with the same volume preserved profiles, completed
  histories and all 40 recorded transcript entries of the interrupted run. It
  reconnected as `runtime_lost`; no worker was restarted.
- Kept a confirmed native WebSocket open during Docker shutdown. The server
  closed it and exited successfully in 5.179 seconds, exercising the drain limit.
- Replaced the volume at the same browser URL. A fresh workspace identity exposed
  no old profiles or runs; restoring the original volume restored its exact
  identity and records. Draft recovery is per tab, not shared across tabs.
- Submitted Save as from an old tab after replacing its volume. The workspace
  guard rejected the new profile name, retained its edited instructions, and left
  the replacement store empty. This tests creation independently of revision CAS.
  After restoring the original volume, ordinary save/revert succeeded and a hard
  reload plus API comparison confirmed the saved profile was restored exactly.
- Checked rejected Host/Origin/Fetch-Site requests, non-JSON writes, and attempts
  to reach native control routes through a UI keepalive connection.

A separate, explicitly labelled simulated failure fixture exercised a confirmed
runtime failure without a durable terminal record. **History incomplete** and
Reload preserved rewind and the recorded node facts. This fixture did not modify
any real ledger and is not counted among the seven provider runs.

## Automated verification

- Frontend: **325 tests**, TypeScript and production Vite build passed.
- Linux Rust **1.97**: the complete UI-enabled workspace passed **1,428 tests**,
  zero failures, 11 ignored. This includes 82 UI service tests. Both UI-enabled and
  default-feature all-target Clippy checks passed with warnings denied.
- The final native binary's separate root boundary lane passed **12 tests** with
  user namespaces required; one explicitly opt-in test was ignored.
- Repository tooling: 34 tests, lint, distribution checks and generated protocol
  checks passed. Formatting, generated CLI reference, Rust documentation with
  warnings denied, and the strict MkDocs build passed.
- The embedded executable and production Dockerfile both built successfully.
  HTTP smoke tests checked assets, bootstrap, profiles and run listing; no
  frontend development server was used for provider or lifecycle tests.

The Mac's Rust 1.96 full native suite encounters unchanged Linux-specific fixture
assumptions. Final native verification therefore used the pinned Linux toolchain;
local macOS behavior was checked with the real providers and browser above.

Two independent implementation reviews checked the standalone boundaries against
[zero-cloud #301](https://github.com/the-open-engine/zero-cloud/issues/301).
Injected services and the selected-run viewer are implemented. Cloud menus, the
shell-less editor entry point/bridge, authenticated adapters and archive expansion
remain the work described in that issue.
