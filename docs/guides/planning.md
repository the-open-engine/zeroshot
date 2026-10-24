# Plan work before running it

A Zeroshot run builds and checks a change against the task you give it, so the result is only as
good as that task. A plan makes the task explicit before any build tokens are spent: what should
change, how each part will be checked, and what must stay as it is.

Plans are Markdown files in your repository. Any coding agent or person can work from them; Zeroshot
is one way to build them.

## Plan with your coding agent

The npm package installs the `zeroshot-plan` skill for Codex, GitHub Copilot, Pi and Claude Code
next to the `zeroshot` skill (see [Install](../getting-started/install.md)). Ask your agent to plan
a change, for example "plan this with zeroshot", and it:

1. reads the request, your repository's agent guidance (`AGENTS.md`, `CLAUDE.md`), the relevant code
   and any linked issue;
2. asks at most three questions per round, each with a suggested default, and only about choices
   that change what gets built or how it is judged;
3. turns vague goals into checkable acceptance criteria, including what must keep working, and
   confirms them with you;
4. proposes sub-plans when parts have different owners, blockers or outcomes;
5. writes the plan to `.zeroshot/plans/<id>-<slug>.md`.

A small, clear change gets a short plan with no questions. If the issue already has a plan in this
format posted by a bot, the skill continues from it instead of starting over. The skill only plans:
it does not edit code or start runs.

The skill is not your agent's plan mode. A plan mode prepares the edit the agent is about to make in
the current session; the skill writes a plan file that outlives the session, so another agent or a
Zeroshot run can build from it later.

## Plan format

A plan has YAML frontmatter and fixed sections. Goal, Acceptance criteria and Constraints are
required; other sections appear only when they have content.

```markdown
---
schema: zeroshot.plan/v1
id: GH-212
title: Double bookings on busy days
status: ready
owner: sam.k
base_commit: 3f2c1a9
depends_on: [GH-198]
sub_plans: [GH-212.1]
source: https://github.com/acme/bookings/issues/212
---

# Double bookings on busy days

## Goal

A repeated Stripe webhook event never creates a second booking.

## Context

- Entry point: `webhooks/stripe.ts` (`handleStripeEvent`); bookings are created in `bookings/create.ts`.
- 14 duplicate bookings since 1 September, all from retries within 2 seconds (support ticket #212).

## Acceptance criteria

- **AC-1** WHEN the same event is delivered again THE SYSTEM SHALL NOT create a second booking.
  Check: end-to-end test, replay one event 50 times, expect exactly one booking.
- **AC-2** WHEN an event is replayed THE SYSTEM SHALL answer HTTP 200. Check: integration test.
- **AC-3** Existing webhook handling keeps working. Check: `npm test -- webhooks`.

## Constraints

- Do not modify `refunds/`.

## Decided approach

Store the event id on each booking with a unique index; on conflict, return the existing booking.

## Open questions

- **Q-1** (non-blocking) Should a peak-load criterion be added? Suggested: yes. Decides: sam.k.

## Decisions

- **D-1** A replayed event returns 200, because Stripe retries any other status. (sam.k)
- **D-2** The unique index is added without downtime, because the table is small. (assumed)

## Out of scope

- Cleaning up existing duplicates: sub-plan GH-212.1.
```

| Field or section                    | Meaning                                                                                                                               |
| ----------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------- |
| `id`                                | The source issue (`GH-212`, `PROJ-123`) or an uppercase key and a number; sub-plans add a level (`GH-212.1`)                          |
| `status`                            | `new`, `needs-input` (a blocking question is open) or `ready`                                                                         |
| `parent`, `sub_plans`, `depends_on` | How plans split, and which must finish first                                                                                          |
| `base_commit`                       | The commit the plan was researched against, to tell when it is stale                                                                  |
| `source`                            | Where the request came from, such as an issue URL                                                                                     |
| Goal                                | The outcome someone can observe or verify: behavior, a measured property, a document or an answer                                     |
| Context                             | Files, entry points and evidence, summarized without personal data                                                                    |
| Acceptance criteria                 | `AC-n` statements, each with a `Check:` naming how it is verified, or `Check (manual):` when it needs a person or another environment |
| Constraints                         | Limits and non-goals the change must respect                                                                                          |
| Decided approach                    | Only what was decided, not alternatives                                                                                               |
| Open questions                      | `Q-n` items with a suggested answer and who decides                                                                                   |
| Decisions                           | `D-n` records of what was decided, why, and by whom (or `assumed` for a reviewer to confirm)                                          |

Ids of criteria, questions and decisions never change once written. A plan is `ready` when no
blocking question is open, every criterion has a check and one criterion covers what must keep
working. Team standards stay in your agent guidance files instead of being copied into each plan.

Criteria fit the kind of change: a bug starts with a test that reproduces it, an investigation is
done when its questions are answered with evidence, performance work names a measured threshold, and
interface work uses screenshots or a visual review. Checks that need production, real devices,
credentials, paid services or a person's judgment are marked `Check (manual):` with who does them;
a run verifies the other criteria and lists these for a person.

Plan files live in `.zeroshot/plans/`, which ignores itself through its own `.gitignore`, so plans
are not committed unless you choose to. Whether work is running or done is tracked by the run, issue
or pull request, not in the file.

## Run a plan

Give the plan's Goal, Context, Acceptance criteria, Constraints, Decided approach and Decisions as
the task of a run, for example through the `zeroshot` skill. The acceptance verifier then judges the
change against criteria you agreed on. Starting the run is the decision to build the plan; nothing
in the plan starts a run by itself.
