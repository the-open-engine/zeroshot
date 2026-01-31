# Postmortem: Detached PR base dropped (2026-01-31)

## Timeline

- 2026-01-31 13:45Z: Fix for detached `--pr-base` merged (PR #258).
- 2026-01-31 14:00Z: PR #259 created for issue #250.
- 2026-01-31 14:00Z–14:20Z: PR branch repeatedly rebased/pushed with commits from `main`.
- 2026-01-31 14:33Z: PR #259 merged after manual cleanup.

## Impact

- PR #259 included many unrelated commits from `main`, confusing review and CI.
- Time lost cleaning the branch and coordinating cluster shutdown.
- Reduced trust in automated PR creation.

## Root Cause

- Detached (`-d`) cluster runs did not forward `--pr-base` and related PR options to the daemon.
- The daemon therefore defaulted to `main` for rebase/PR base, even when `--pr-base dev` was supplied.

## Contributing Factors

- Cluster was started from a local checkout that did not include the 13:45Z fix yet.
- No single source of truth for run options in daemon mode; new flags required manual env wiring.
- No guardrail to alert when daemon options differ from CLI intent.

## Detection

- User observed PR #259 contained unrelated commits and questioned the base branch.

## Resolution

- Killed the active cluster to stop further pushes.
- Rebuilt the PR branch from a clean `origin/dev` base and re-opened PR #259 on `dev`.
- Merged fix PR #258 to preserve `--pr-base` in detached runs.

## Prevention

- Forward all run options via a single `ZEROSHOT_RUN_OPTIONS` payload in daemon mode.
- Parse daemon run options in `buildStartOptions` as a fallback for any missing CLI flags.
- Add unit coverage to ensure env-run-options are honored.
- Update operator docs to explicitly require forwarding options for detached runs.

## Action Items

- [x] Preserve `--pr-base`/`--merge-queue`/`--close-issue` in detached runs (PR #258).
- [x] Document daemon option forwarding in `AGENTS.md` and `CLAUDE.md`.
- [x] Add `ZEROSHOT_RUN_OPTIONS` fallback for daemon runs and unit tests.
- [ ] Add a preflight warning when local branch is behind remote base (optional follow-up).
