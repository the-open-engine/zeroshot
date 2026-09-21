# Cloud workspace campaign 301 evidence

The implementation and release order are defined in
[the approved plan](cloud-workspace-ui-301.md). This record separates executed
checks from remaining local/provider/release/deployment work.

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
- UI unit suite: 336 passed; TypeScript and Vite build passed.
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

Local Cloud/provider/browser, canonical release, released-source repin, dev
deployment and deployed browser tests have not yet run.

### Analysis coverage

Opcore Verify has no native-source diagnostics. The final UI findings are reviewed
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

### Worktree recovery

The app-managed sibling Zeroshot worktree disappeared during implementation. All
written changes were recovered from exact edit proposals and verified in the locked
`/home/ec2-user/dev/zeroshot-cloud-workspace-301` worktree. The approved plan and
pre-commit fixture isolation fix had already been pushed. Subsequent test results
above refer to the recovered working tree.
