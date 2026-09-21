# Cloud workspace UI, issue 301

Status: senior design/spec review approved on 2026-09-21; no remaining
`BLOCKING-SPEC` or `BLOCKING-CORRECTNESS` findings. Implementation may proceed.

Authority: [zero-cloud issue 301](https://github.com/the-open-engine/zero-cloud/issues/301),
approved by the user for full implementation. The user requires Zeroshot work first,
then zero-cloud; local Cloud browser tests with provider runs precede the Zeroshot
release, Cloud release repin, Cloud dev deployment, and deployed browser retest.
Both repositories start on `codex/cloud-workspace-ui-301`, based on current
`origin/main`. Push each reviewed phase to its feature branch. Cloud `main` pushes
automatically deploy dev, so keep its PR unmerged until the release order below permits it.

## Acceptance and scope

The issue's acceptance clauses are:

> Same workspace and history contract in local binary, target image and cloud; no cloud-specific graph interpretation.
>
> Test stale saves, edits during save, unresolved fields and document switching.
>
> Open an active run midway, replay from its start, return to live, reconnect, and continue after archival; graph visits and outputs match throughout.
>
> Verify access loss, partial/expired archives and shutdown with an active stream. UI assets and archived replay remain available after the run container is gone.

The identifiers below refer to the corresponding sections of the same
[approved issue](https://github.com/the-open-engine/zero-cloud/issues/301).

| Clause | Required behavior | Owning phase |
| --- | --- | --- |
| I: Interface and ownership | Cloud owns authentication, menus, selection, storage, navigation and theme. Zeroshot owns one draft, pending edits, undo, graph inspection and playback. Rust owns catalog, authoring, validation and admitted history. | 1, 2 |
| B: Host bridge | Pinned same-origin shell-less mount independent of run containers; versioned ready/init, profile/run open, save snapshot/ack, dirty/validation and theme/navigation messages. Validate sender/origin and correlate workspace/document/request. History uses authenticated APIs. | 1, 2 |
| P: Profiles | Revision tokens and conditional save; commit field edits or report unresolved edits. Acknowledge only the captured snapshot. Protect switches and drop foreign import revisions. Scope recovery/writes to user/org/workspace and reject changed authority. | 1, 2 |
| H: Definition/history | Versioned, credential-free admitted graph/runtime/input/source revision and ordered events with execution/iteration identities, safe transcripts and native control visits; retain projection version and never rebuild from current profiles. | 1, 2 |
| L: Live/archive | Expand post-termination 30-day archive. One run-scoped browser API supplies definition, pages and resumable follow; preserve execution identities/cursors across target/archive handoff. Keep Cloud status cursors separate. Add BFF streaming. | 2 |
| A: Availability/access | Separate current status, durable events and archive collecting/complete/incomplete/expired. Reconnect/handoff preserves playback and gaps are explicit. Preserve problem codes. Reuse authorization and keep target/provider credentials server-side. | 1, 2 |

Continuous archival during execution is explicitly outside scope. Also excluded:
new infrastructure, dependencies, services, controllers, signers, identity systems,
generic bridge/stream frameworks, a Cloud graph interpreter, parallel editor code,
profile format migrations, archive retention changes, production deployment, and
unrelated changes to CLI run/watch/log behavior.

## Existing code and decisions

Zeroshot `ui/src/App.tsx` already owns the draft and pending-edit lifecycle;
`profile-save.ts` acknowledges one document generation. `workspace-services.ts`
and `run-history-source.ts` separate transport from graph components. Extend these
boundaries instead of making another editor.

`zeroshot/src/profile_ui/runs.rs` reads admission snapshots and emits version 1
definitions, projection version 1, ordered `v2:` execution cursors, string execution
IDs and native control records from `runs/control.rs`. It already distinguishes
unpersisted runtime failure from durable terminal snapshots. Its API and authoring
helpers currently depend on the optional `ui` feature; extract the native service
surface so Cloud's existing `zeroshot` Rust dependency can use it without embedding
`ui/dist` in the API build. The `ui` feature continues to own static assets and the
standalone browser boundary.

Cloud profiles in `billing/run_intents/profiles.rs` currently upsert without CAS.
The existing archive stores bounded watch/log chunks with checkpoints in
`run_archives`/`run_archive_chunks`; extend that worker and object store. Existing
Cloud watch/status cursors do not identify the native execution ledger.
`frontend/lib/bff/downstream.ts` calls `response.text()`, so follow needs an explicit
stream response path through the same BFF authorization and fixed egress boundary.

### Shared contract

1. Add one version-1 bridge module owned by Zeroshot, plus the Cloud host adapter.
   Use `ready`, `init`, `open_profile`, `open_run`, `request_save`, `save_snapshot`,
   `save_ack`, `state`, `theme` and `navigate` message kinds. Pre-init `ready`
   carries version/request identity; `init` binds the host workspace and authority
   scope. Every later message carries the bridge version, workspace identity and a request ID;
   document operations also carry a document ID/generation. Validate
   `event.source === parent` or the selected iframe window, exact same origin,
   expected message shape and current identities. `init` supplies only same-origin
   service paths, authority scope and theme. It contains no target URL or token.
2. Reuse the existing draft/save helpers for profile opens and snapshot requests.
   Flush valid pending edits; answer unresolved edits without saving. The host
   performs the conditional write and acknowledges the exact snapshot/request.
   A late acknowledgement never replaces another document or clears later edits.
   New/imported documents have no revision. Dirty, pending-field and in-flight
   transitions protect profile/run/org navigation. Cloud owns the outer menus;
   standalone keeps its current small toolbar around the same workspace.
3. Cloud derives workspace authority from the authenticated user, organization and
   profile scope. Bind browser recovery and the open draft to that identity.
   Conditional writes include the captured authority and expected revision; the
   server compares authority with current authentication before writing. Neither a
   new active organization nor a replacement session can redirect an old draft.
   A revision changes for every content writer, including default-profile refresh.
   On conflict, no profile or default-selection change commits.
4. Reuse existing definition and page wire records, including `version: 1`,
   `projectionVersion: 1`, `control`/`controlError`, safe logs, source revision,
   string IDs and native `v2:` cursors. Expose `POST /native-v2/history/definition`
   with `{runId}` and `POST /native-v2/history/page` with `{runId, after}` on the
   existing private target authority, authenticated through its current capability.
   This export is private-capability-only: target-wide hosted authentication alone
   does not grant per-run access. Cloud applies existing run authorization before
   using the server-held capability.
   Validate the run ID and bounded body, rejecting unexpected fields. Keep cursors
   in JSON request bodies so the raw target HTTP parser needs no new
   query surface. Reads use the already-owned controller/ledger and never
   create/recover a controller. Private/hosted targets
   continue to reject the standalone `/ui/` mount.
5. Cloud supplies authenticated run-scoped definition/history/follow endpoints.
   Poll the bounded target page export for live data; use retained native pages
   after target removal. Archive the immutable definition once and execution
   history plus native control records through the existing post-termination
   worker. Preserve native event IDs/cursors; filter previously delivered events
   at chunk boundaries and reject missing/inconsistent sequences. A definition
   comes only from the admitted export or its retained copy. Include both new
   retained components in archive completion and target teardown predicates;
   preserve the existing lease, checkpoints, checksums, limits and collection
   deadline. Earlier archives lacking these components report unavailable or
   incomplete, without reconstructing definitions from profiles.
6. Keep archive availability outside the immutable definition and distinguish it
   from current run status and durable history completeness. An observed runtime
   failure can precede the final durable event; `finished` alone cannot end Cloud
   follow during archive collection. Continue from the accepted execution cursor
   across reconnect/handoff, keep the viewer's selected playback position, and
   report incomplete/expired/unavailable history with its problem code. Update
   `RunHistoryView`'s shared follow predicate, which currently stops when a run is
   terminal, so it follows observation completeness instead. This metadata changes
   observation lifecycle only; native graph projection remains unchanged.

Browser follow uses the shared bounded fetch-based SSE reader to preserve HTTP
problem bodies as well as structured stream errors; native EventSource cannot
expose rejected handshake codes. Preserve `Last-Event-ID` resume behavior,
backpressure, cancellation and bounded incomplete-frame buffering. Forward only
the declared stream content and safe errors through BFF; do not reuse the ordinary
buffered JSON timeout for the life of a stream. Disconnect and host shutdown drop
owned readers/timers. Recheck existing authorization during follow and stop on
access loss, without acquiring new credentials in the browser.

Permit same-origin frames only on Cloud workspace host routes, where the current
CSP denies frames. The embedded document permits pinned same-origin assets and
its existing ELK worker; retain the surrounding auth/session response protections.

## Work packages and file ownership

Each worker owns its package and focused tests. Coordinate shared entry points
before editing; reviewers classify findings as `BLOCKING-SPEC`,
`BLOCKING-CORRECTNESS`, `HARDENING`, or `OUT-OF-SCOPE`. Only the first two require
implementation. Push after reviewer approval and applicable checks.

| Package | Files/components | Deliverable and clauses |
| --- | --- | --- |
| Z-native | Zeroshot `zeroshot/src/profile_ui*`, extracted native workspace/history module, `native_v2_target_authority/{transport,transport_http}.rs`, target composition, required `lib.rs`/Cargo feature wiring; focused native tests | Reusable authoring/catalog/validation and one native history export; authenticated private reads and status/history separation. H, A. |
| Z-workspace | Zeroshot `ui/src/{App,AppHeader,RunHistoryBoundary,RunHistoryView}*`, workspace bridge/services/save/storage, history source/stream, `main.tsx`, Vite entries, `ui/README.md`, relevant tests | Shell-less entry, snapshot protocol, host-independent navigation, structured HTTP/follow errors; standalone uses same components. I, B, P, H, A. |
| C-services | Cloud `backend/services/api-gateway/src/billing/{public_api,run_intents}*`, existing target runtime HTTP adapter, one next-numbered migration and `schema.sql`, generated contracts and native tests | Atomic profile revisions/authority check; native authoring adapters; run definition/pages/follow; archive definition/history checkpoints and availability. P, H, L, A. |
| C-shell | Cloud `frontend/components/platform/{profiles,run-control}*`, `frontend/lib/{platform,bff}*`, route-specific `frontend/lib/security/csp.ts`, thin App Router routes, existing hosted build/prepare scripts, frontend image/compose build wiring and frontend/browser tests | Pinned static workspace mount, host menus/selection/save/theme, streaming BFF and browser services. I, B, P, L, A. |

Keep protocol additions in Zeroshot if Rust wire types must change. Regenerate
protocol/contracts with repository generators; don't hand-edit generated output.
The C-services worker owns the migration and contract generation to avoid conflicts.
Within Z-native, the coordinator owns catalog/data/outcomes extraction and
`lib.rs`/Cargo wiring while the native worker owns history and target transport.
Reuse the existing `RunLedger` interface where access to the already-owned
controller ledger replaces the concrete standalone SQLite reader.
Build static workspace assets from the same exact Zeroshot source revision as the
target/runtime and Rust crates, using the existing source preparation mechanism;
bundle assets into the Cloud frontend image. No request-time asset fetch from a run
target or mutable upstream branch. Update repository guidance only for changed facts.

Complexity budget: four implementation packages in the existing components, one
bridge version, one execution-history export, one browser history adapter, one BFF
stream mode, and at most two additive migrations if phase separation requires it.
No new npm/Cargo package dependency or deployed service. File splits for the native
service extraction stay within its owner. Stop for scope review if work needs a new
authority, protocol family, durable state machine or infrastructure, or if resolving
review would require a second architecture cycle. A reviewer suggestion does not
expand issue 301.

## Phases and checks

**Phase 1: finish Zeroshot (I, B, P, H, A).** Z-native and Z-workspace may work
concurrently after agreeing on the small contract. Build the UI before testing the
optional native `ui` feature. Run focused Rust and UI unit tests while editing,
then private-target HTTP and standalone interface tests. Review save snapshot and
stream lifecycle behavior before pushing this phase. No Cloud implementation
starts until the Zeroshot contract and phase review pass.

**Phase 2: integrate Cloud (all clauses).** First pin Cloud's existing runtime lock
and Cargo dependencies to the immutable pushed Zeroshot feature-branch commit for
local validation. This is temporary integration provenance; canonical repinning
comes after release. C-services and C-shell may proceed against the reviewed
contract. Add CAS, native API adapters and archive expansion; finish the host/mount
and streaming path. Run unit/static checks, followed by focused Postgres/API/BFF
integration and interface tests. Obtain senior approval before full-stack testing.

**Phase 3: validate the complete behavior.** Use one testing ladder:

| Gate | Evidence |
| --- | --- |
| Static/unit | Zeroshot `npm --prefix ui test`, `npm --prefix ui run build`, Rust fmt/Clippy and affected crate tests; Cloud frontend lint/typecheck, backend checks and affected unit tests. Run Opcore Verify and targeted Sense after interface changes; report unsupported coverage. |
| Integration/interface | Authenticated target export and native authoring equivalence; concurrent stale saves, edits during save, unresolved fields, import/switch and authority-change rejection; live/archive cursor continuity; problem codes survive HTTP and stream adapters. |
| Full affected lanes | Zeroshot Rust/UI and protocol/distribution checks required by touched paths; Cloud applicable unit/integration/interface suites, generated contract check and repository quality. Run existing full-stack e2e/API/CLI gates. No Kubernetes code changes are planned, so kind gates don't apply. |
| Local browser/provider | Start isolated local Cloud with its existing tooling. Open a provider-backed active run midway, replay from `v2:0`, inspect execution outputs/control visits, seek back, return live, reconnect, then replay after archive collection and target removal. Test profile cases in the browser too. Record run ID, source/asset revisions, final execution cursor and screenshots; never record credentials. |
| Focused failure cases | Use deterministic fixtures for access loss, partial/expired archive and failure before durable final event. Interrupt an active stream and shut down the serving stack; verify prompt cleanup and resumable playback. Only inject failures tied to these acceptance clauses. |

Reserve local instance `campaign301` and, after confirming availability, ports
`FRONTEND_PORT=3301`, `BACKEND_PORT=18301`, `POSTGRES_PORT=55301` through
`scripts/local-dev.sh`; do not reset other local instances. Existing defaults may
already be occupied. Provider runs are authorized for validation; keep them bounded
to the required evidence. If device authorization is needed, first start the
isolated D-Bus shell and ephemeral Secret Service described by host guidance, then
keep all credential-using commands in that shell without `set -e`.

A failed test is not a passed gate. Fix harness failures and validate the harness
before rerunning; avoid repeated stack/cluster runs without a concrete correction.
Keep a short evidence record with each gate's command, result and limitation.

## Release and deployment order

1. Complete phases 1–3, including local Cloud browser/provider evidence, and push
   reviewed work. Keep the Cloud PR open because merging triggers dev deployment.
2. Merge the approved Zeroshot PR through its normal flow and publish its canonical
   release using `.github/workflows/release.yml`. Verify native assets, target image
   and immutable release source; the release source must include the reviewed work.
3. Replace Cloud's temporary integration pin in `hosted/zeroshot-runtime.lock.json`,
   `Cargo.toml` and `Cargo.lock` with that exact released Zeroshot commit. Rebuild the
   workspace assets from the same revision and verify existing source-lock checks.
   Repeat the affected integration checks and local smoke only where the released
   bytes or pin changed; preserve the full earlier evidence.
4. Merge the reviewed Cloud PR only now. Follow `.github/workflows/deploy-dev.yml`,
   verify its deployment and acceptance journey, then repeat browser editing and
   active/archived run playback against dev. Verify the deployed source/asset pin
   and replay availability after container removal. Record any environment blocker
   separately; don't label an unexecuted deployed test as passed.
