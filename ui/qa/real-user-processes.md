# Real-user profile authoring exercise

These process descriptions are the requirements, written before creating the profiles.
All ten profiles will be authored and saved through the graphical interface, without JSON editing,
API profile creation, or running the processes. Agent instructions describe work to perform; profiles
that prepare communications or operational plans stop at a human-reviewable deliverable.

This record covers successive editor iterations. Counts, saved-profile audits, and browser checks
belong to their recorded iteration; later follow-ups describe changes to that behavior.

## 1. Security patch through merge — `security-patch-merge`

Implement a narrowly scoped security fix in a repository, review both the requested behavior and
regression/security risks, repair rejected changes, and use the native merge delivery worker.
Delivery problems return to repair and review; unrecoverable worker failures stop the process.
The native software-change merge template is the baseline, with security-specific worker and
reviewer instructions. Inputs are the requested change and optional source issue. Shared data includes
review diagnostics and the acceptance reviewer's current change title/description. Output is the
native authoritative merge receipt. The review/repair cycle is bounded at ten rounds.

## 2. Database migration package — `database-migration-package`

Prepare an expand/migrate/contract migration for a named database, without deploying it. First assess
the requested schema change and write a migration proposal. Independently check compatibility and
rollback safety. A rejected review triggers revision; a passing pair allows preparation of the SQL,
verification queries, rollback instructions, and operator checklist. Stop after four review rounds.
Inputs: task, database engine, affected tables, permitted downtime in minutes. Shared fields hold the
proposal and review feedback. Output is the migration handoff package for a human operator.

## 3. Flaky-test investigation — `flaky-test-investigation`

Investigate a list of failing test suites separately. For each suite, collect a reproduction and
classify likely nondeterminism, test isolation, or product failure. Combine the suite findings into
an investigation report, propose a minimal repair, and have a verifier check evidence and whether
quarantining tests would hide a real defect. Revise the report/repair plan up to three times.
Inputs: task, test command, failure-log location, suite names. Each map item is one suite; collected
findings feed the final report. Output is an evidence-backed triage and repair plan, not a deployment.

## 4. API compatibility release — `api-compatibility-release`

Assess an API change against its existing clients, then prepare implementation and documentation
work in parallel. Review compatibility and migration guidance independently. A failed review sends
the work back for correction; approval produces a release checklist and draft changelog. No release
is published automatically. Inputs: task, API version, client list, compatibility policy. Shared data
includes the compatibility assessment and reviewer feedback. Output is a reviewable release package.

## 5. Service incident handoff — `service-incident-handoff`

Turn incident notes into a coordinated response plan. Build the technical timeline and customer
impact assessment in parallel. Classify whether immediate escalation is needed, then take the
appropriate escalation-planning or routine-follow-up branch. Draft an internal handoff and review
it for evidence, ownership, and unsupported claims before producing the final package.
Inputs: task, service name, incident notes, severity threshold. Shared data contains timeline,
impact, classification, and draft text. Outputs are the handoff and recommended next actions;
this graph does not page anyone, send messages, or modify production systems.

## 6. Customer onboarding plan — `customer-onboarding-plan`

Convert a customer's stated goals and requested products into an implementation plan. Review the
requirements for missing information; if necessary, prepare a clarification checklist. Otherwise,
work through the requested products individually, then develop training and rollout plans in
parallel. A final verifier checks owners, dependencies, and measurable acceptance criteria.
Inputs: task, customer goals, requested product names, target date. Item scopes describe one product
at a time. Output is a human-reviewable onboarding plan and any unresolved questions.

## 7. Product launch content kit — `product-launch-content-kit`

Create a coherent launch kit from a product brief. Produce website copy, an email draft, and a
social-post draft in parallel. Review factual claims and brand voice independently, repair rejected
content, and package approved drafts with a channel checklist. Limit revision to three rounds.
Inputs: task, product facts, audience, tone, forbidden claims. Shared fields hold the draft kit and
review feedback. Output is a content handoff; nothing is published or sent.

## 8. Conference planning packet — `conference-planning-packet`

Develop venue/logistics, agenda, and budget proposals in parallel for a small conference. Check
whether the combined proposal fits the attendee count, date, accessibility requirements, and budget.
Revise an infeasible proposal up to three times. Prepare the final itinerary and decision log only
when the plan is feasible. Inputs: task, city, date, attendees, budget, accessibility requirements.
Output is a planning packet; the graph makes no reservations, payments, or vendor commitments.

## 9. Customer support drafting — `customer-support-drafting`

Classify a support request as a product question, billing question, or suspected defect. Follow the
matching drafting/investigation path, then review the draft for correctness, empathy, and promises
that the support team cannot make. Revise a rejected answer within a bounded loop. Inputs: task,
product, customer message, account context without secrets. Shared fields hold the classification,
response draft, and review feedback. Output is a proposed reply and escalation note for a person to
approve; no customer message is sent.

## 10. Editorial evidence review — `editorial-evidence-review`

Check each proposed factual claim against the supplied research material, collect the findings,
and draft an article outline. Run evidence-quality and clarity reviews in parallel. Revise weak
claims or unclear framing, then prepare an editor handoff containing the outline, source notes,
and unresolved questions. Inputs: task, audience, research brief, claim list. Each map iteration
has independent claim data. The final output is a reviewable editorial package, not publication.

## Acceptance criteria and results

For every process: build a meaningfully distinct workflow through visible controls; give activities
real instructions; configure realistic run inputs and data routing; choose explicit runtime values;
save a named profile; and verify native validation and persisted content. Record UI problems,
repairs, and any remaining gap below as testing proceeds. Never claim a process was executed.

All ten profiles are saved in the local user profile store and are available in the profile picker at
`http://127.0.0.1:4185/ui/`. They contain **186 graph nodes and 53 executable bindings** in total:
52 agent bindings and one native Git delivery binding. Every profile has an explicit Codex/OpenAI
runtime and `gpt-6-astra` agent model; the delivery binding uses the native Git worker contract.

The graph definitions, instructions, schemas, mappings, conditions, runtime choices, and saves were
created through visible browser controls. Native template selection and the UI's ordinary import/export
controls were permitted; no JSON editor, external profile generator, or API profile write was used.
Read-only retrieval and native validation were used afterward to audit what the UI actually saved.

### Saved profiles

Node counts include groups, decisions, and terminal nodes. Executable counts include agents and the
single native delivery worker; they are not counts of executions or model calls.

| Profile                      | Nodes / executables | Saved process and deliverable                                                                                                                                                                                                                                                                   |
| ---------------------------- | ------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `security-patch-merge`       | 20 / 6              | Native software-change merge graph with security-specific implementation, acceptance, code review, and repair instructions. Ten review rounds; native merge receipt output.                                                                                                                     |
| `database-migration-package` | 15 / 4              | Prepare migration SQL and evidence, then independent compatibility and rollback reviews with up to four repair/review rounds. Approved files remain in `artifacts/database-migration/`; the success payload is null.                                                                            |
| `flaky-test-investigation`   | 14 / 3              | Map up to ten suite records into collected findings, then compose and review the investigation report for up to three rounds. Returns `investigationReport`.                                                                                                                                    |
| `api-compatibility-release`  | 20 / 6              | Parallel read-only implementation/compatibility planning and migration-documentation planning, followed by a single writing agent and independent parallel reviews. Up to three rounds; release checklist, changelog, and migration guide in `artifacts/api-release/`; success payload is null. |
| `service-incident-handoff`   | 16 / 6              | Parallel timeline and impact assessment, followed by an explicit urgency classifier, a shared response planner, drafting, and review. Separate urgent/routine successful terminal paths return the same authoritative urgency enum, handoff, response plan, and review feedback.                |
| `customer-onboarding-plan`   | 20 / 6              | Requirements assessment can return a clarification checklist early. Otherwise map up to ten product records, build training and rollout plans in parallel, then compose and verify onboarding. Returns either questions/requirements or the onboarding plan/review feedback.                    |
| `product-launch-content-kit` | 17 / 6              | Parallel website, email, and social drafts, followed by independent factual and brand reviews. A repair plan drives up to three writing rounds. Returns all three drafts and a channel checklist.                                                                                               |
| `conference-planning-packet` | 17 / 5              | Sequential venue, agenda, and budget Agents write plan files, followed by a feasibility Verifier for at most three rounds. Handle failures and exhaustion explicitly, then assemble the final packet file and return only `packetPath`.                                                         |
| `customer-support-drafting`  | 31 / 7              | Classify product, billing, or suspected-defect cases; each branch has its own draft/review loop, capped at three rounds, with branch-local failure and exhaustion handling. Returns the proposed reply, escalation note, and review feedback.                                                   |
| `editorial-evidence-review`  | 16 / 4              | Map up to twelve claim/source records into findings, then compose an outline and run evidence/clarity reviews in parallel for up to three rounds. Returns the outline, editor packet, and both reviews' feedback.                                                                               |

### Fictional example inputs

These examples explain how the saved schemas can be used. They are fictional documentation examples,
not values inserted into profiles, observed facts, or submitted runs. File references identify material
that a future caller would need to supply. State fields such as plans, drafts, and review feedback are
produced inside the graph rather than entered by the caller.

1. **Security patch through merge.** `task`: “Fix cross-tenant invoice access in the invoice download
   handler. Preserve administrator access, add regression coverage for a user from another tenant,
   and review error responses for information leakage.” Optional `issueNumber`: the string “482”.
   The caller supplies the actual repository and delivery authorization when starting a run.

2. **Database migration package.** `task`: “Move customer notification preferences from a nullable
   legacy column into a separate preference table without breaking either application version.”
   `databaseEngine`: “PostgreSQL 16”; `affectedTables`: customer_accounts, notification_preferences,
   and notification_jobs; `allowedDowntimeMinutes`: the integer 2. The requested package should make
   its backfill, dual-read/write transition, rollback limits, and validation queries reviewable.

3. **Flaky-test investigation.** `task`: “Explain the intermittent checkout and session failures and
   propose the smallest defensible repair; do not quarantine a product defect.” `testCommand`:
   “npm run test -- --runInBand”; `failureLog`: “fixtures/ci/failed-test-runs.log”. `suites` contains
   three item records whose `name` values are “checkout.clock-boundaries”, “session.concurrent-refresh”,
   and “inventory.reservation-expiry”. Each item's name is routed into that suite's investigation;
   the collection of findings reaches the report writer and evidence reviewer.

4. **API compatibility release.** `task`: “Add cursor pagination to the order-list API while existing
   offset clients continue to work during a documented transition.” `apiVersion`: “v2”; `clients`:
   “web-dashboard 3.4”, “partner-sdk 1.8”, and “warehouse-exporter 2.1”; `compatibilityPolicy`:
   “Preserve existing response fields and offset semantics for 90 days; unknown request fields must
   not change results; publish a migration guide and test both pagination modes.” No release is
   published by this graph.

5. **Service incident handoff.** `serviceName`: “invoice-download-service”; `severityThreshold`: the
   integer 50, meaning affected customers, not an ordinal severity score. `incidentNotes`: “At 09:12
   UTC, invoice downloads began returning intermittent 502s. Support has 62 distinct affected customer
   reports; the precise start time and total scope are unknown. At 09:24 a responder reduced traffic
   to one worker pool, but recovery has not been verified. No evidence of data loss has been collected.
   Prepare proposed response owners and the next evidence/update checkpoint.” This profile's saved
   input schema has these three fields and no separate `task` field.

6. **Customer onboarding plan.** `task`: “Prepare an implementation and enablement plan for an
   85-person distributor across six offices.” `customerGoals`: “Reduce manual stock reconciliation;
   sales and warehouse operations need a shared order view. Name proposed business and technical
   owners and identify access, import, and training dependencies.” `products` contains records whose
   `name` values are “Inventory”, “Order management”, and “Reporting”; `targetDate`: “2026-11-02”.
   Omitting important requirements in the goals should lead to the clarification deliverable.

7. **Product launch content kit.** `task`: “Prepare a reviewable launch kit for Harbor Notes.”
   `productFacts`: “The fictional product offers shared meeting notes, editable action lists, CSV
   export, and a 14-day trial. The supplied brief makes no security certification or measured
   productivity claim.” `audience`: “Operations managers at small manufacturing companies”; `tone`:
   “Calm, specific, and practical”; `forbiddenClaims`: “zero setup”, “guaranteed savings”, and
   “certified carbon neutral”. The outputs are drafts and a checklist, not published content.

8. **Conference planning packet.** `task`: “Develop a one-day engineering conference proposal with
   two talk tracks, a workshop, breaks, and a closing discussion.” `city`: “Berlin”; `date`:
   “2027-03-18”; `attendees`: the integer 80; `budget`: the number 24000, with EUR specified in the
   task or planning assumptions; `accessibility`: “Step-free access, accessible toilets, live
   captioning, a quiet room, clear dietary labeling, and space for wheelchair users in both tracks.”
   The budget field is a number and does not encode a currency separately. The three proposals are
   files in `artifacts/conference-planning/`; the final output identifies
   `artifacts/conference-planning-packet.md` through `packetPath`.

9. **Customer support drafting.** `task`: “Prepare a reply for human approval and a useful escalation
   note.” `product`: “Harbor Notes”; `accountContext`: “Fictional company account, 25 seats, renewed
   this month; no credentials or payment details”; `supportPolicy`: “Do not promise refunds, fix
   dates, or data recovery. Acknowledge uncertainty, request the minimum evidence, and identify the
   appropriate internal owner.” For `customerMessage`, use one of three realistic cases: “How can a
   project owner export action items?” (product), “Our invoice lists 30 seats although the admin page
   shows 25; please explain before renewal” (billing), or “Two people edited a note at once and the
   older version appeared after refresh; we can provide timestamps” (suspected defect).

10. **Editorial evidence review.** `task`: “Prepare an evidence-led outline about the fictional pilot
    program, retaining uncertainties.” `audience`: “Operations leaders considering a small pilot”;
    `researchBrief`: “Supplied materials describe a 20-person pilot, 18 completed exit interviews,
    and self-reported reporting effort. Distinguish self-report from measured effects and do not
    infer causation.” `claims` contains records with both `text` and `source`: “Participants reported
    less time preparing weekly reports” / “research/pilot-summary.md, table 2”, and “Every pilot
    participant completed the study” / “research/participant-log.csv”. The second proposed claim is
    deliberately questionable against the brief; the source files would need to be supplied and
    checked before accepting it.

### Architecture and authoring notes

- The profiles retain the native structured graph model. The workflow view flattens sequences for
  reading; group state and promotion boundaries remain part of the saved graph.
- The original exercise used read-only Verifiers for parallel planning. Its API profile placed
  implementation and file writing in one sequential Agent after the parallel plans. This records
  that iteration's runtime constraint and example design, not an editor requirement: the current
  editor permits parallel writing Agents, with target admission/execution owning runtime support.
- Required string State fields omitted from Run inputs initialize to empty strings. Draft and
  feedback slots use that native pattern with explicit execution-error and review-outcome handling;
  caller-owned numbers, arrays, and other constraints remain real run inputs. Collected Map arrays
  are optional until a successful collection supplies them.
- Map inputs use object item records with explicit field paths: suite/product `name`, and editorial
  claim `text` plus `source`. Maps keep item state independent and collect results in input order.
  A Success node inside a map body would end the whole process; these collection bodies are
  executable workers, with outcome handling after the map.
- The incident profile runs `classify_urgency` after its parallel assessments. That verifier is the
  sole classification authority: its signal writes an optional `urgency` enum directly to root
  state. An analysis-error guard precedes the shared planner, drafting, and review sequence; those
  three consumers require the exact enum, and both successful terminals return it. Urgent and
  routine paths use the classifier's original signal. Native Choice has no aggregate worker-error
  control, and conditional branch workers cannot be referenced by a common guard as though both
  had executed. The shared continuation avoids that ambiguity without duplicating its steps.
- The support profile intentionally has three case-specific loops. Each branch owns the relevant
  instructions, review criteria, and error guards, so worker results dominate their guards without
  a fabricated no-op or an early Success that would end the process.
- Database and API deliverables are workspace files with a null terminal payload. The conference
  profile returns only the path of `artifacts/conference-planning-packet.md`; the other handoff
  profiles return their structured text fields directly.
- The conference draft was exported through the UI and imported unchanged through the UI to load an
  updated build while the browser's unsaved-page handling blocked reload. No exported JSON was
  edited or generated externally. A temporary test-driver issue where helper closures retained the
  previous tab was corrected; that was not an application defect.

### Conference revision: plans as files

The original brief above requested parallel proposals. At the user's request, the saved conference
profile now uses files for the proposals and final packet. This revision was authored and saved
entirely through the UI in a separate tab, preserving the user's unsaved editor tab.

- `venue_logistics`, `agenda_design`, and `budget_scenarios` are writing Agents in that sequence.
  They replace their complete `venue.md`, `agenda.md`, and `budget.md` files under
  `artifacts/conference-planning/` each round. Later planners read the earlier plans. Their structured
  outputs are null, with no plan-body state writes. The revision used the then-current native shared
  workspace's exclusive writer contract; the later parallel-writer runtime work is separate. These
  authored file dependencies also explain this example's sequence and do not require other graphs
  to serialize their writers.
- The read-only feasibility Verifier checks the files against original inputs and rejects missing,
  empty, stale, or inconsistent proposals. Its `verdict` controls the loop; `reviewFeedback` carries
  concise corrections, limited to 1200 characters by instructions. This is a prompt limit, not a
  schema-enforced bound. Planner errors, review errors, exhaustion, and packet-writing errors retain
  explicit failure routes.
- The final Agent writes `artifacts/conference-planning-packet.md` and returns only `packetPath`.
  Root state initializes that string slot, and nested groups inherit it. None of the worker inputs
  include this output slot. Full `venuePlan`, `agendaPlan`, `budgetPlan`, and `planningPacket` fields
  have been removed from schemas, mappings, and promotions.
- This removes repeated full document bodies from structured node inputs and outputs. Provider
  messages, command text/stdout, and tool results can still contain file contents; file-based plans
  do not guarantee their absence from logs. The returned path identifies a workspace file and does
  not automatically publish a downloadable artifact. No planning run was started or files generated.
- The revision passed native validation and an independent saved-profile audit: 17 nodes, five
  runtime bindings, correct sequential dependencies, all failure routes retained, and no legacy plan
  fields remaining. Reloading and reopening it through the profile picker displayed “Profile valid.”

### Issues found and fixed

- **Map item authoring:** arrays can now contain visually editable object items with flat fields.
  Deeper imported schemas stay preserved, and replacing object items requires confirmation.
- **Input wiring:** “Use parent state for input” copies the schema and connects required fields as
  one explicit edit. Optional fields remain optional and unbound; ordinary schema copy retains
  its previous meaning.
- **Worker access:** the editor explains workspace write access and supports lossless conversion
  from an Agent to a read-only Verifier for parallel planning.
- **Loop stop conditions:** the picker offers Verifiers within the loop body and keeps excluded
  existing selections visible for repair. Native validation still checks whether the selected
  verifier executes on every completing iteration.
- **Workflow connections:** controlled node updates retain React Flow measurements and handle
  geometry, avoiding disappearing or detached edges after edits.
- **Large decision captions:** common error and approval conditions are summarized from the intact
  guard before text truncation. The launch workflow now displays “Any error” for its five-worker
  failure path; selecting that path still opens all five original conditions.
- **Renaming and placement:** renaming moves the device-layout entry and removes the old key, so
  a newly created node cannot inherit the renamed node's occupied position. Advanced structure
  view also separates overlapping stored sibling cards on entry without modifying graph data.
- **Native Map errors:** an admitted map with a failed item previously raised
  `MissingSelectedValue` while collecting output, before its authored error route could run.
  Reduction now tracks each item's own writes and promotes only complete collections. Missing
  output does not become null, a shorter array, or a stale inherited array. Valid empty collections
  still produce an empty array; malformed successful durable output still fails closed.
- **Profile review repairs:** caller-owned migration constraints are supplied directly to the
  independent reviewers and repair worker. Incident urgency is carried from one explicit classifier
  through genuine signal-to-state bindings into every consumer and successful output, with matching
  enum types and an error guard before consumption. Error guards and loop exits distinguish rejected
  work, exhausted budgets, and worker execution failures instead of treating them as equivalent.

### Validation and limits

- All ten saved profiles passed native validation (HTTP 200 with a valid result), and independent
  read-only inspection checked their stored instructions, input bindings, outputs, budgets, and
  runtime bindings.
- All ten were reopened through the browser profile picker and displayed “Profile valid.” Every
  Workflow card was rendered without hidden cards or nonfinite coordinates. Collapse/expand,
  Advanced dragging, and Arrange were exercised; dragging moved the Map's Y position from 12 to
  approximately 48.67, and Arrange restored 12. Both formerly overlapping incident analyst cards
  selected the correct named inspector. Light, dark, and system themes were checked, then restored
  to System. Browser warning and error logs were empty.
- The final UI suite passes **171 tests**. TypeScript, the static UI build, and the native UI build
  pass.
- The affected pure-reducer lane passes **37 tests**: 25 reducer cases, ten boundary cases, one
  failure case, and one architecture case. Four new Map regressions cover worker errors,
  ordered/empty collection, inherited collection data, and malformed successful durable output.
- Fresh independent reducer probes passed nine cases: the four Map regressions, four variants
  using a prefilled `[98, 99]` results field, and a failed item with a still-pending sibling. These
  are deterministic synthetic histories, not live model or workflow runs.
- Final independent checks passed all six native `profile_ui` tests, the profile-store identity/default
  regression, and all 25 reducer tests. The whitespace/diff check, Prettier checks for UI and
  documentation including AGENTS.md, and workspace-wide `cargo fmt --all -- --check` passed.
- Native Clippy remains blocked by two pre-existing `nonminimal_bool` errors in the Claude and
  Codex command builders. Those unrelated files were not changed for this exercise.

No profile was executed. No model output quality, external tool integration, actual merge, deployment,
publication, message delivery, reservation, or payment is claimed. Browser authoring, persisted graph
admission, and pure reducer behavior are the verified scope of this exercise.

### Workflow-only editor follow-up

The separate Advanced structure view and its unused canvas, layout code, styles, and tests were
removed. Run inputs and data now opens directly from the toolbar. Containing-group links in the
inspector reach sequence scopes, and loop/map body replacement and wrapping remain available there.
Toolbar Add activity targets the root while the inspector is closed, matching the absence of a
visible selection.

Computer-use checks created a temporary unsaved graph with a loop and map, added body agents,
replaced bodies, wrapped a single body in a sequence, navigated containing groups and scoped data,
and exercised undo/redo. The temporary graph was discarded without saving. Saved conference and
merge profiles reopened, including the native delivery inspector, run inputs, and runtime controls.
The root insertion dialog identified `run` with no inspected selection. Browser warning/error logs
were empty. An independent source review checked the removed view's authoring paths; the child had
no browser access, so all computer-use checks were performed by the primary agent.

The remaining 163 UI tests, TypeScript, frontend build, native UI build, formatting, and whitespace
checks pass. No saved profiles were changed by this follow-up, and the user's unsaved tab was preserved.

### Clean outcomes follow-up: implementation

The editor now presents standard failure handling as settings and ordinary completion as a compact
Run result endpoint. The native graph remains the execution contract: no default policy is added to
the protocol or reducer, and viewing an existing profile does not rewrite it.

- `workflow-outcomes.ts` recognizes full worker-error sets (`timeout`, `crash`, `malformed`,
  `refusal`), pure `any` combinations, valid `k_of_map` error thresholds, and known loop exhaustion,
  map overflow, and unmet parallel-join guards that route directly to Fail. Error-only decisions
  collapse to their existing otherwise content. Mixed decisions keep their meaningful paths and
  original branch indices. Partial error sets, business signals, unfamiliar or malformed guards,
  and custom recovery remain explicit.
- Folded policies retain their original checkpoint, evaluation priority, failure reason, guard and
  run/parallel-branch boundary. A policy shown with a worker or group is still evaluated at its
  authored checkpoint; it does not become an immediate global error rule. Inspector settings keep
  the underlying condition and scoped data available for editing.
- An adjacent Verifier and Choice can share the Verifier's outgoing paths when they are in the
  same sequence and the visible conditions read that Verifier's declared signals. Edges retain the
  Choice owner and branch index. This does not move bindings, flatten state, or cross a parallel join.
- Run result retains the successful terminal's schema and output bindings. Explicit early finishes
  remain visible, and terminals inside a parallel branch are labelled End branch rather than global
  run completion. A parallel group whose join cannot be reached still exposes its group result and
  continuation; no terminal-to-join edge is invented.
- `POST /ui/api/authoring` is a draft-only native operation for completion, supported stop-on-error
  handling, and failure-reason changes. Rust lowers these actions into ordinary Succeed, Choice and
  Fail nodes and returns graph/runtime data. Saving and native admission remain separate operations.
  New simple work receives these explicit guards and completion where structurally supported;
  custom conditional/repeated paths are not replaced by a new global failure rule.
- Adding work to an ordinary completed workflow inserts it before the existing final completion,
  preserving the result contract. Add next follows an existing standard error continuation so the
  preceding worker's error check still happens before the new work. Explicit early/failure paths
  are not silently extended beyond their terminal.
- The editor permits writing Agents in parallel and retains authored joins and branch scopes. It
  neither converts them to read-only Verifiers nor serializes them. The user is enabling parallel
  writers in the runtime separately; earlier sequential/read-only examples record their original
  design and are not a current UI restriction.

### Clean outcomes follow-up: browser verification

The primary agent tested the built native UI with computer use on 2026-09-15. Independent agents
reviewed projection, native authoring, and integration; their findings prompted fixes for delayed
settings responses after inspector unmount, edits during failure-reason submission, and appending
after parallel groups with branch-local terminals. Children had no browser access.

- Opened conference Run settings and inspected all four folded policies, their original reasons,
  checkpoint labels, full conditions, and priority/scope guidance. A reserved failure reason was
  rejected by Rust without changing the graph. Applied a valid temporary reason and checked undo,
  redo, and restoration of the original value.
- Removed one error label through the visual condition editor. The now-custom decision and failure
  endpoint reappeared on the canvas. Undo restored the folded policy. Run result still opened the
  `packetPath` schema and its connection from `prepare_planning_packet`.
- Created a temporary blank draft entirely through the UI. Adding the first Agent authored ordinary
  completion and a stop-on-error policy despite incomplete draft runtime fields. Add next preserved
  the prior checkpoint and created a second Agent policy. Wrapped the second Agent in Parallel and
  added another writing Agent. The group policy refreshed to include both `write_brief` and
  `write_schedule`, with its check after the group. Both remained writing Agents. Checkpoint
  navigation retained access to the folded policy. The incomplete runtime draft was discarded;
  this check makes no claim about executing concurrent writers.
- Opened, arranged, and re-saved all ten examples using the visible controls and save shortcut:
  `api-compatibility-release`, `security-patch-merge`, `database-migration-package`,
  `flaky-test-investigation`, `service-incident-handoff`, `customer-onboarding-plan`,
  `product-launch-content-kit`, `customer-support-drafting`, `editorial-evidence-review`, and
  `conference-planning-packet`. Each displayed **Profile saved** and **Profile valid**. The visible
  workflows retained business/review outcomes, maps, loops, and Git delivery/repair activities.
- A read-only before/after comparison confirmed all ten saved graph/runtime documents were
  unchanged: the refreshed canvases fold 32 standard failure routes without changing execution.
  The original user tab and its draft were preserved. Conference planning was left open in the
  updated saved-example tab; final browser warning/error logs were empty.

Validation: 178 UI tests, 17 native profile-UI tests, TypeScript, frontend/native UI builds,
Rust formatting, and whitespace checks pass. Native Clippy still encounters the two pre-existing
`nonminimal_bool` errors described above. No profiles were executed.

### Inputs and outputs follow-up

The normal inspector now uses Inputs with source selectors and a single Outputs section. Separate
state, binding, promotion, error-policy, and parallel-join controls are removed from normal authoring.
The editor creates the underlying typed routes and error checks. Graph JSON remains available and
existing custom behavior is preserved. The menu info icon opens **How workflows behave**, a dedicated
page for the defaults; the editor contains no restriction tutorial. The page preserves the current
draft, selection, and undo history. Reload also retains the selected saved profile.

Review loops create work, review, and feedback for the next attempt. Fixed-count loops return the
last round, maps collect in order, parallel work waits for all branches, and decisions require a
compatible result on every continuing path. Optional results keep their absence until proven present.
The reducer tracks writes within the current scope so failed or skipped final producers cannot
promote stale results. Writing Agents remain selectable inside parallel groups and maps.

All ten examples were edited and saved through the visible UI, without JSON editing or profile-store
writes. The changes remove unused inputs, including future results and agents' own outputs. Real
review feedback remains connected to the previous attempt. Caller inputs, runtime settings, topology,
and instructions were retained; conference planning still writes its plans to files.

| Saved profile              | Inputs removed | Agents updated |
| -------------------------- | -------------: | -------------: |
| security-patch-merge       |              2 |              2 |
| database-migration-package |              7 |              3 |
| flaky-test-investigation   |              6 |              3 |
| api-compatibility-release  |             22 |              5 |
| service-incident-handoff   |             22 |              6 |
| customer-onboarding-plan   |             32 |              6 |
| product-launch-content-kit |             29 |              6 |
| conference-planning-packet |              1 |              1 |
| customer-support-drafting  |             13 |              7 |
| editorial-evidence-review  |             14 |              4 |
| **Total**                  |        **148** |         **43** |

Read-only snapshots before and after the UI edits confirmed these counts. All ten saved documents
were fetched again after the final native rebuild, matched the saved snapshots, and passed native
validation.

### Browser regressions and independent checks

A separate browser tester built a scratch workflow through actual UI interactions: a producer and
consumer, renamed and removed outputs, a review loop with feedback, and a map with object items. The
scratch graph reached **Profile valid** after its runtime was supplied. Opening the defaults page and
returning preserved the draft. The tester's browser became unavailable after resuming the session, so
the primary agent completed the remaining checks in a separate temporary in-app tab.

- Renaming a root input and adding an Agent no longer introduces invalid bindings on group nodes.
- Output renames retain downstream input aliases and update their selected sources. Type edits
  propagate through map item schemas and consumer inputs. Deleting the collection leaves
  **Choose collection** and **Choose source**, retaining the typed consumer row; Undo restores it.
- An invalid output name displays an error. Escape restores the name and clears the error.
- The final type-edit check found an additional stale schema in generated post-map continuation
  groups. A fix propagates owned storage types through these scopes, with regressions for carried
  declarations and scope isolation. The rebuilt browser test reached **Profile valid** both before
  and after String → Number, and after deleting then undoing the collection. The scratch draft was
  discarded through the UI and the primary agent's temporary tab was closed.
- The info page was inspected in the browser, including its layout and return navigation. A saved
  conference profile remained selected after reload. Review-loop captions read **Approved** and
  **Try again · up to 3 attempts** only when the underlying guards prove that behavior.
- A fresh verifier audit found a conditional integer write incorrectly dominating a later number
  write in one Choice branch. The proof now widens overlapping facts conservatively. A regression
  rejects the unsafe integer consumer while accepting the number consumer.
- The reducer architecture check exposed schema interpretation in the promotion request. Presence
  metadata is now derived once from the verified root; the architecture test remains unchanged.

The UI suite passes 211 tests; TypeScript and the frontend/native UI builds pass. The native editor
suite passes 35 tests, the graph verifier passes 82 plus 11 boundary tests, and the
reducer passes 25 cases plus ten boundaries, one failure case, and its architecture check. Protocol
artifact verification and Rust documentation pass. The unsafe-promotion fixture was regenerated
through the Rust testkit to check an actual required read without proof of producer success.

The full workspace suite is not green on this macOS host. Remaining failures include process fixtures
that reference unavailable `/usr/bin/cat`, local-controller socket paths exceeding the platform limit,
hosted merge-plan status, and streaming process checks. Full Clippy also reports existing unused
imports in platform-dependent tests and boolean-expression warnings in unchanged provider/test code.
These failures are recorded separately from the passing affected editor, verifier, and reducer lanes.
No saved profile was executed and no model, delivery, or concurrent-writer run is claimed.

### File-producing workers: revision brief

Review all twelve saved profiles, including `code-change` and `software-change-merge`. Authors of
plans, research notes, reports, drafts, and implementation changes must be writing Agents. Verifiers
check evidence or classify a decision. Keep only file paths, short review/classification notes,
outcomes, and native Git delivery metadata in structured results.

- API release: parallel implementation/documentation planning files, one implementation writer,
  independent reviews, and a file-based repair plan.
- Flaky tests: independent suite investigators write isolated evidence files; a report writer reads
  the returned paths and revises its report from evidence review.
- Incident: parallel timeline and impact files feed urgency classification, response planning,
  handoff writing, and independent review.
- Onboarding: readiness classification precedes per-product plan files; training and rollout
  writers run in parallel before composition and review.
- Support: preserve exclusive product/billing/defect branches; each writer produces reply and
  internal-note files, then receives concise review feedback.
- Editorial: independent claim researchers write evidence files; an outline/packet writer uses
  those files, followed by parallel evidence and clarity reviews.
- Launch: parallel website/email/social files feed a kit/checklist writer and parallel reviews.
  A repair-plan writer prepares the next round when rejected.
- Conference: venue and agenda proposals run independently in parallel, stating assumptions.
  Budget analysis follows both; feasibility review coordinates revisions before packet assembly.
- Code change, software-change merge, security patch, and database migration: audit the existing
  source/package writers, parallel reviewers, and repair paths. Preserve native delivery contracts
  and the exact software-change merge structure.

Mapped writers allocate a fresh directory atomically for each item, including duplicated display
names. They return paths in Map order. Downstream work reads only these current paths, never a glob
that could include stale artifacts. Parallel writers own separate files; dependent writing remains
sequential. These are authoring changes and validation checks, not executed model runs.

### File-producing workers: saved results

All twelve profiles were reviewed. Eight were updated through the visible editor; no profile JSON
or profile-store files were edited. The four existing software/native profiles already use writing
Agents for implementation and repair. Their graphs and runtimes are unchanged, including the exact
software-change merge contract.

| Updated profile            | Verifiers converted to Agents | Final shape                                                                               |
| -------------------------- | ----------------------------: | ----------------------------------------------------------------------------------------- |
| api-compatibility-release  |                             3 | Parallel planning files, implementation, reviews, repair-plan file                        |
| conference-planning-packet |                             0 | Existing venue and agenda Agents grouped in parallel; budget follows both                 |
| customer-onboarding-plan   |                             4 | Unique per-product files, parallel training/rollout plans, composition, review            |
| customer-support-drafting  |                             3 | Exclusive reply/note writers followed by actual category reviews                          |
| editorial-evidence-review  |                             2 | Unique claim-evidence files, outline/packet writer, parallel reviews                      |
| flaky-test-investigation   |                             2 | Unique suite evidence files, report writer, evidence review                               |
| product-launch-content-kit |                             4 | Three parallel draft writers, new kit/checklist Agent, parallel reviews, repair-plan file |
| service-incident-handoff   |                             4 | Parallel timeline/impact files, urgency classification, response plan, handoff, review    |
| **Total**                  |                        **22** | **Plus one new writing Agent for launch assembly**                                        |

Writer outputs contain required file paths. Reviews/classifiers retain only decisions and concise
feedback; native delivery metadata remains unchanged. All three Maps allocate fresh item directories,
including for repeated display names. The launch assembler receives the existing previous-attempt
repair-plan path and reads it only when nonempty; it never treats a leftover file as current feedback.
Both launch reviewers check the assembled index and checklist alongside the channel files.

Real editing exposed and fixed four issues:

- Role conversion and grouping: an explicit Verifier-to-Agent action preserves regular outputs,
  routes, and runtime identity while refusing live review-only channels. Independent contiguous
  activities can be grouped in parallel, and group ends expose Add next.
- Rapid input selection: pending source edits now visibly lock their controls. Overlapping events
  report an error instead of silently dropping an input.
- Required paths: native source binding creates a proven success checkpoint before a dependent
  reader. The launch draft's existing source-error cases were factored forward while retaining its
  review/retry routes and source failure reason. Custom recovery and competing failure priority
  remain protected; regression tests cover rejected rewrites and the requested consumer's default.
- Final failure decisions: the verifier no longer expands unrelated output conditions for an
  immediate all-Fail Choice. Undefined controls, invalid labels, exhaustiveness, and the original
  assignment ceiling remain checked.

The final browser save and read-only snapshots confirm all twelve profile identities and caller
input schemas are unchanged. Existing runtime selections are unchanged, all 22 conversions are
present, and all twelve saved profiles pass native validation after the rebuilt server restarts.
An independent agent reviewed every profile and the new native authoring paths; its final review
found no remaining issue, including the launch feedback provenance and distinct failure reasons.
The rebuilt browser reopened the saved launch profile as valid. Attempting to convert its active
brand reviewer to a writing Agent was rejected without changing the profile. The defaults page
showed the file-writing convention, and the original onboarding tab was refreshed from saved data;
the temporary editing tab was closed.

Validation passes: 238 frontend tests, TypeScript, the frontend/native UI builds, 49 native UI tests,
84 graph-verifier tests, and 11 verifier boundary tests. Clippy still reports the pre-existing
boolean-expression warnings in the Claude/Codex command adapters; the executable build reports an
existing platform-dependent dead-code warning. No profile, model, or delivery run was started.
