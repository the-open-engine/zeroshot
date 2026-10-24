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
3. turns vague goals into checkable acceptance criteria and confirms them with you;
4. proposes sub-plans when parts have different owners, blockers or outcomes;
5. writes the plan to `docs/plans/<id>-<slug>.md`.

A small, clear change gets a short plan with no questions. The skill only plans: it does not edit
code or start runs.

The skill is not your agent's plan mode. A plan mode prepares the edit the agent is about to make in
the current session; the skill writes a plan file that outlives the session, so another agent, a
teammate or a Zeroshot run can build from it later.

## Plan format

A plan has YAML frontmatter and fixed sections. Goal, Acceptance criteria and Constraints are
required; other sections appear only when they have content.

```markdown
---
schema: zeroshot.plan/v1
id: WR-2
title: Double bookings on busy days
status: ready
owner: sam.k
parent: WR
depends_on: [WR-1]
sub_plans: [WR-2.1]
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
- **AC-2** WHEN an event is replayed THE SYSTEM SHALL answer HTTP 200. Check: acceptance review.

## Constraints

- Do not modify `refunds/`.

## Decided approach

Store the event id on each booking with a unique index; on conflict, return the existing booking.

## Open questions

- **Q-1** (non-blocking) Should a peak-load criterion be added? Suggested: yes. Decides: sam.k.

## Decisions

- **D-1** A replayed event returns 200. (sam.k)

## Out of scope

- Cleaning up existing duplicates: sub-plan WR-2.1.
```

| Field or section                    | Meaning                                                                          |
| ----------------------------------- | -------------------------------------------------------------------------------- |
| `id`                                | Short unique id: an uppercase key and a number; sub-plans add a level (`WR-2.1`) |
| `status`                            | `new`, `needs-input` (a blocking question is open) or `ready`                    |
| `parent`, `sub_plans`, `depends_on` | How plans split, and which must finish first                                     |
| `source`                            | Where the request came from, such as an issue URL                                |
| Goal                                | The outcome, as behavior someone can observe                                     |
| Context                             | Files, entry points and evidence, summarized without personal data               |
| Acceptance criteria                 | `AC-n` statements, each with a `Check:` naming how it is verified                |
| Constraints                         | Limits and non-goals the change must respect                                     |
| Decided approach                    | Only what was decided, not alternatives                                          |
| Open questions                      | `Q-n` items with a suggested answer and who decides                              |
| Decisions                           | `D-n` records of what was decided and by whom                                    |

Ids of criteria, questions and decisions never change once written. A plan is `ready` when no
blocking question is open and every criterion has a check. Team standards stay in your agent
guidance files instead of being copied into each plan.

A plan file describes work still to do. Commit it with the change that implements it, or on its own
in a pull request when others should agree on it first. The change that completes the plan deletes
the file; git history keeps it. Whether work is running or done is tracked by the run, issue or pull
request, not in the file.

## Run a plan

Give the plan's Goal, Context, Acceptance criteria, Constraints and Decided approach as the task of
a run, for example through the `zeroshot` skill. The acceptance verifier then judges the change
against criteria you agreed on. Starting the run is the decision to build the plan; nothing in the
plan starts a run by itself.
