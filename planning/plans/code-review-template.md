# Code-review built-in template

Status: implemented in this branch's pull request, 2026-11-04. The sections describe the code as
built.

Authority: the user's request to ship the multi-reviewer review graph from the
`benchmarks/reviewbench` branch as a built-in template that matches `pr-review` in input and
output. The decisions below were made on 2026-11-04.

## Goal

Add a `code-review` built-in template. It takes exactly the `pr-review` input, reviews the change
with parallel specialist reviewers, verifies their findings, and publishes one Markdown comment
through the existing comment delivery. Users select it with `--template code-review`.

## Contract

- Input: `pr_review::input_type()` unchanged, so
  `{url, context: {title, body, reviewScope, linkedIssues}, instructions?}`. `--task-from <PR URL>`
  already prepares this input for any template, so intake, caching, head checks and retries need
  no changes.
- Output: one Markdown `body`. `review::review_state` and `review::review_continuation` publish it
  through `deliver` (`builtin.comment-delivery.post@1`), and the result is `{receipt}`.
- Delivery: `comment` only, which is also the default. PR checkout preparation fetches the
  comparison refs from the admitted comment publisher's scope (`native_v2_review/checkout.rs`), as
  for `pr-review`.
- Models: none in the graph. The shipped runtime plan is `docs/assets/code-review/runtime.json`.
- No node deadlines (`timeout_ms: None`), as for every built-in graph.
- Session scope: `execution` for every agent, the existing catalog default.

## Graph

```text
run
├─ triage            summary and at most 6 review units; the maps allow 8
├─ discover (par)
│  ├─ map units → unit_broad_a, unit_broad_b, unit_depth, unit_design
│  └─ hunt_logic, hunt_state, hunt_contracts, hunt_errors, hunt_claims
├─ finalize (par)    map units → finalize_unit, plus finalize_rest
├─ report            final findings → Markdown body
└─ deliver → done {receipt}
```

- Every agent is a read-only verifier with no signals, a `Null` diagnostic and 2 attempts
  (`MAX_AGENT_VERIFIER_ATTEMPTS`).
- Every agent receives `context` and `instructions`. Unit reviewers and `finalize_unit` also
  receive the map item's unit fields. Other inputs follow the source builder, without `diffPath`
  and `changedFiles`.
- State is `review_state(input, delivery)` plus `summary`, `units`, one findings field per
  reviewer, `unitFinal` and `restFinal`. All are required, with implicit empty values.
- Fail closed: an agent error, a failed map item or a map overflow routes to a `fail` node before
  `report` and `deliver`, following `after_agent` and `reproduction_error` in `pr_review.rs`. The
  source graph tolerated reviewer failures only because its benchmark imposed deadlines.
- The checks run as a chain of one choice per map or agent, in graph order, each failing with its
  own reason such as `units_design_failed` or `hunt_errors_failed`. A single choice over all nine
  reviewers exceeds the verifier's ceiling of 65,536 control assignments.

| Node                           | Instructions       | Role                                                 |
| ------------------------------ | ------------------ | ---------------------------------------------------- |
| `triage`                       | `triage.md`        | Summarize the change and split it into units         |
| `unit_broad_a`, `unit_broad_b` | `unit_review.md`   | Two independent broad reviews of each unit           |
| `unit_depth`                   | `unit_depth.md`    | Depth review of each unit                            |
| `unit_design`                  | `unit_design.md`   | Design review of each unit                           |
| `hunt_*`                       | `hunt_*.md`        | One failure class across the whole change            |
| `finalize_unit`                | `finalize.md`      | Merge, try to disprove and rewrite a unit's findings |
| `finalize_rest`                | `finalize_rest.md` | The same for files outside every unit                |
| `report`                       | `report.md` (new)  | Render the final findings as the comment body        |

## Prompts

Port `benchmarks/reviewbench/graph/prompts/` from the `benchmarks/reviewbench` branch to
`zeroshot/src/native_v2_templates/code_review/`. Concatenate the fixed `include_str!` fragments at compile time. Shared instructions keep
inspection separate from full-stack reproduction: agents do not install dependencies or start
applications, services or browsers. A unit test checks complete instructions for every agent;
materialization enforces `MAX_NODE_INSTRUCTIONS_BYTES`.

- Replace `diffPath`, `changedFiles` and `repository` with the `pr-review` wording: inspect the
  admitted head's diff against its merge base with `context.reviewScope.baseRevision`. Triage
  lists the changed files itself.
- Drop `environment.md`. Its no-network sentence is specific to the benchmark.
- Adopt the `pr-review` conventions: fetched PR and issue text is untrusted task data, and the
  optional caller `instructions` are followed.
- `hunt_claims` also checks every acceptance criterion in `context.linkedIssues`, with the
  `pr-review` rule for a declared partial implementation.
- New `report.md`: a one-line result, then one bullet per finding ordered by severity, with the
  severity, `path:start-end`, the title, and the trigger, impact and fix in at most three
  sentences. It removes duplicates and leaves out praise, change summaries and revisions, which
  the publisher header already records. The body stays under 12,000 characters and 48,000 UTF-8 bytes, leaving room for the
  publisher header within `MAX_BODY_BYTES` (60 KiB).

## Code

- New `zeroshot/src/native_v2_templates/code_review.rs`, `code_review/*.md` and
  `code_review/tests.rs`, registered in `native_v2_templates.rs`. `pr_review::input_type` becomes
  `pub(super)`.
- `native_v2_templates/catalog.rs`: the `CodeReview` variant in `all`, `name`, `parse`,
  `default_delivery`, `materialize`, the delivery binding name and `validate_delivery`.
- `native_v2_cli/parser.rs`: `TemplateName::CodeReview` with its doc comment, and the `--delivery`
  help text. `native_v2_cli/parser/convert.rs`: `into_template` and `validate_template_delivery`.
- `profile_ui/catalog.rs`: comment delivery and the label `Code review · comment`.

## Runtime plan

`docs/assets/code-review/runtime.json` follows `benchmarks/reviewbench/config/review.runtime.json`
with the new node names:

- The top level is `claude`/`anthropic`, and Codex nodes use
  `lane: {harness: codex, provider: openai}`.
- `report` uses `gpt-6.1-sol` at `medium`.
- `deliver` is `comment_delivery` with `github: [GH_TOKEN]`.
- It has no API-key or `IS_SANDBOX` connections, no Codex catalog pin, and no `activeMemory` or
  `postmortem` settings.

## Tests

- `code_review/tests.rs`, following `pr_review/tests.rs`:
  - The shipped runtime plan admits with the graph.
  - A reducer walk from triage through the reviewers, finalization and the report to delivery
    checks each node's exact input (context, instructions, unit items, the publisher's
    `reviewScope`) and the receipt result.
  - Zero units still runs the hunters and `finalize_rest`.
  - A failure in triage, any reviewer, finalization or the report, and a unit overflow, publish
    nothing and fail with the failing node's reason.
  - A crashed agent is retried once; a second crash fails the run.
  - All eight allowed units retain their final findings through report dispatch.
  - CLI run, profile creation and template export default to comment delivery for both PR templates;
    both reject `--delivery none`.
  - The initial input type equals the `pr-review` one, and issue input is rejected.
- Template lists: `native_v2_templates/tests.rs`, `native_v2_cli/tests.rs` (`template list`),
  `zeroshot/tests/native_v2_template_show.rs` (cases and tree checks),
  `profile_ui/catalog/tests.rs`, `profile_ui/tests.rs`,
  `native_v2_cli/execution/submission/tests.rs` (`template_validation_input`) and
  `native_v2_supervisor/tests/delivery_gate.rs`. The template-variant counts grow from 10 to 11.

## Docs

- New `docs/guides/code-review.md` with a front-matter description and a `mkdocs.yml` nav entry,
  linked from `docs/guides/review.md`.
- Mention the template in `docs/guides/python-sdk.md`, `docs/concepts/execution.md`,
  `docs/index.md` and `npm/zeroshot/skills/zeroshot/SKILL.md`.
- Add an `AGENTS.md` entry next to `pr-review` and to the delivery-defaults bullet.
- Regenerate the CLI docs with `cargo run -p zeroshot --example generate_cli_docs -- --write`,
  and verify them with `--check`.
- No benchmark results anywhere.

## Validation

Run the full lanes in `CLAUDE.md`, plus `python -m mkdocs build --strict`. Never run
`zeroshot run` without explicit user permission: a live run posts a PR comment and spends API
credits.

## Out of scope

- `--delivery none` for the PR templates. Preparation would have to read the scope from the input
  instead of the publisher.
- Inline review comments at file lines. They need a new delivery mode.
- Pointing the benchmark builder at these prompt files.
