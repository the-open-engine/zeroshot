# Run history browser review

Reviewed 17 September 2026 using native Chrome accessibility controls and screenshots through computer use. A separate QA window left the user's in-app editor tab untouched.

## Real runs exercised

| Run                                                | Result    | Checked                                                                                                                                |
| -------------------------------------------------- | --------- | -------------------------------------------------------------------------------------------------------------------------------------- |
| Replay study: incident handoff review loop         | Succeeded | Collapsed graph, nested expansion, two review visits, distinct transcripts and decisions, execution selection, jump to execution start |
| Replay study: parallel launch packet (current CLI) | Succeeded | Run switching, parallel group presentation, final status, light/dark presentation                                                      |
| Replay study: parallel launch packet               | Failed    | Failure reason, both attempted writers, recorded provider crash and continuation diagnostics                                           |

The incident run was opened again after the first revision: sequence wrappers were absent from the collapsed view and the incorrect finished-run refresh footer was gone.

## Passed through the UI

- Opening history presents a collapsed graph with no authoring, connection, reorder, or editable JSON controls.
- Expanding the incident loop and parallel reviews exposes selectable activities. Manual expansion survives timeline navigation.
- Selecting the first and second code-review visits shows their respective recorded diagnostics and transcripts.
- Jumping to an execution's start removes its future decision and output; its transcript is empty at that cursor. Going to the start of the run removes all future executions and counts.
- The slider accepts keyboard navigation. Replay advances, pauses, and stops at the final event; Next and Latest are disabled at the end.
- The Messages filter hides activity records. Long command output expands and collapses.
- All three real runs load after switching between them; the inspector resets for the newly selected run.
- An unknown run URL shows an unavailable-history message and Retry, without exposing profile-recovery controls.
- An unsaved temporary profile draft survives Profiles → Runs → Profiles with its name and Unsaved state intact. The temporary draft was then discarded through the UI. No saved profile was modified.
- System/dark and light themes retain readable graph, inspector, and timeline presentation. The original System preference was restored.
- At approximately 575px window width, the run-list drawer opens, selection closes it, empty search reports no matches, clearing search restores results, and timeline controls remain available.

## Findings corrected during iteration

- Sequence wrappers required several expansions before revealing meaningful activities. History now flattens all sequences, as the editor does; only Loop, Map, Parallel, and Choice groups initially collapse.
- Native completed runs were mistakenly offered snapshot refresh. Native phase normalization corrected the completed-run footer; this was rechecked in Chrome.
- Resizing the window or opening the inspector could leave the graph outside its reduced viewport. The read-only canvas now refits after its container size settles.
- The run-details button wrapped into a separate row at narrow widths. The history header now prevents wrapping and constrains its title.
- Missing control observations were rendered as “Not started,” despite recorded descendant activity. Absent observations now have no badge; settled group activity uses a neutral “Recorded activity” badge with an execution count, without claiming control completion.

## Static review

Run selection aborts the previous history load. Each awaited detail/page is checked against both cancellation and its load generation before updating state. Switching back to Profiles unmounts the history view and aborts its owned load. A failed page retains the successfully loaded prefix and cursor for retry. Cursor merges reject gaps and conflicting events rather than silently presenting incomplete history as complete.

History has its own error boundary, so its recovery does not remove the editor draft. Run configuration and graph definitions are read-only, and projected decisions/transcripts come from the selected history prefix rather than the final snapshot.

No further blocking issue was found in the load/cancellation or responsive code review. An intentionally delayed in-flight request was not injected through the browser; cancellation was reviewed statically, while ordinary run switching was tested through the UI.

## Final in-app browser review

The root reviewer completed the remaining coverage in a separate in-app browser tab, preserving the user's editor tab.

- Opened all ten simulated histories and visually inspected their initial collapsed graphs. Each loaded with the expected completed or failed timeline; simulated provenance is explicit.
- Expanded Editorial evidence review's claim map and selected its three items. Each had its own recorded inputs and finding path; the second item exposed its 12,432-character transcript through the long-output control.
- Expanded Customer onboarding plan's empty product map. Its worker had no execution, transcript, or invented output; the following parallel writers and assembly had recorded activity.
- Confirmed Flaky test investigation ends with its recorded failure reason.
- Rechecked neutral group counts, flattened sequences, Agent/Verifier labels, and the inspector's execution selector in the final build.
- Opening the inspector refits the graph to the smaller canvas. At 575 × 800, the header stays on one row, the run drawer selects and closes, and the timeline remains usable. Restored the default viewport afterward.
- Reopened the real incident review loop, selected the first rejected review and second accepted review, then jumped to the first review's start. Its future decision and transcript disappeared while expanded groups remained open.
- Confirmed finished native runs have no snapshot-refresh footer. Left the real incident review loop available in Runs.

Final validation: 260 UI tests passed; frontend and UI-enabled Rust builds passed. Native checks passed 57 profile-UI tests, 13 ledger tests, and 86 CLI tests. Rust formatting and generated CLI documentation checks passed. Clippy remains blocked by two pre-existing `nonminimal_bool` diagnostics in the Claude and Codex command adapters.

## Inspector refinement

A 25.5-second recording of the incident handoff replay was captured before these changes, followed by its rejected and accepted review scenes. The MP4 was decoded successfully after encoding; it is a local artifact, not a run-ledger or repository asset.

- Replaced execution dropdowns with a horizontal mini-timeline. Checked pointer selection, Left/Right and Home navigation, automatic scrolling of the six-execution run timeline, and separate map-item stops.
- Input precedes Output, including returned fields, verdicts, review diagnostics, and artifacts. Workers with no recorded return say so instead of fabricating file contents from their commentary.
- Generic value rendering preserves named parent paths, sibling groups, arrays, empty values, and literal text. Thirteen focused tests cover singleton chains (including array items), bounded rendering, escaping, exact text, and malformed JSON.
- Transcripts use a separate bounded scroll box. Browser measurements confirmed a 358px viewport over 8,599px of transcript content; scrolling moved only that box, leaving the inspector's scroll position unchanged. Expanding the 12,432-character example output kept the same box height.
- Rechecked the two real review visits: rejected feedback and accepted feedback remain distinct; jumping back to the second review's start removes its future verdict and output.

The updated UI suite passes 273 tests; frontend and UI-enabled Rust builds pass.

## Node playback

- Added compact play/pause and replay-from-start controls beside the execution timeline; removed
  the invocation's Jump to start button. Node playback moves the shared run cursor through its
  recorded activity, including descendant executions and provider output drained after completion.
- In the browser, replaying Acceptance from the end returned to Visit 1 running, with no future
  verdict and an empty transcript. Pause held that position; resume reached the accepted second
  visit and stopped. Expanded graph groups stayed open.
- At the global start, no executions are exposed, but node playback remains available. Starting
  there selects the node's first start. Manual timeline navigation and closing the inspector stop
  playback. A node with no recorded execution has disabled playback controls.
- The root's controls replay from the beginning of the run. The compact controls were visually
  checked at 1280 × 720 in the system dark theme.
- A fresh reviewer caught concurrent executions finishing out of start order: automatic inspector
  selection now follows the current event instead of the latest start. Focused tests cover this,
  repeated visits, descendant scopes, unreached nodes, late output, usage and prefix visibility.

Validation: 276 UI tests passed; frontend and UI-enabled Rust builds passed. No new runs were started.
