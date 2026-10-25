---
name: zeroshot-graph-design
description: Design or revise a Zeroshot graph through a guided interview grounded in the invoking repository. Review an ASCII diagram and exact node instructions, select runtimes, then confirm the profile name and local or platform storage scope before saving.
---

# Zeroshot graph design

Help the user design a lean graph they understand and can reuse. Keep the conversation short and opinionated: inspect what you can, propose sensible defaults, and ask about choices that materially change the design. Graph design does not authorize starting a run.

## Confirm the starting point

Begin by asking:

> I'll assume you want a reusable code-change graph for the repository this skill was called from: `<invoking directory>`. Is that right, or do you have a different purpose or repository in mind?

Use the actual invoking directory. Wait for the answer before inspecting the repository or proposing a graph. If the user already confirmed this in the conversation, continue without repeating the question. Honor any specific task or different purpose they provide; otherwise keep the task as a per-run input.

Do not ask for harnesses, models, or effort yet.

## Inspect and propose

Read applicable repository guidance, manifests, scripts, CI workflows, test configuration, and enough representative code to identify the stack and normal development workflow. Use read-only inspection; designing a graph does not require running the test suite or changing application code.

Infer what the user would almost always check after a code change. Ground the proposal in concrete commands and file paths, and label uncertain assumptions. Distinguish routine checks from checks that depend on the affected code:

- Use the repository's own build, formatting, lint, type, and test commands where applicable. Scope expensive suites according to its established workflow.
- For a UI, consider browser interaction, visual inspection, accessibility, and existing end-to-end tests. Identify the main flows worth checking; do not add a browser node merely because a dependency mentions a UI framework.
- For services, CLIs, libraries, or infrastructure, choose the corresponding integration, smoke, compatibility, or deployment validation supported by the repository.
- Translate common manual verification into concrete agent actions when tools permit. If a required check needs unavailable services, credentials, or human judgment, surface that dependency and ask how to handle it. Missing evidence cannot count as a pass.

For code-change graphs, ask: "How should runs deliver changes: leave them local, push a branch, open a PR, or merge?" Reuse any earlier choice and include delivery in the proposal.

Propose a complete first draft using the evidence. Ask focused follow-ups only where the answer would change the graph; avoid making the user describe facts you can inspect.

## Keep the graph lean

Start with an implement/repair worker followed by independent review, looping on actionable feedback until the required reviewers accept. Combine responsibilities when one reviewer can cover them well; add nodes only for distinct work or verification that earns a separate pass.

Parallelize independent work and reviews. Keep overlapping edits and checks that compete for mutable resources sequential unless they have suitable isolation. Avoid routing trees, coordinator layers, and separate planning or reporting agents without a concrete need.

The worker must receive every relevant rejection. Repairs trigger fresh review of the resulting workspace; success requires all required reviewers to accept that version. Keep environment failures distinct from code defects.

Write concise node prompts with the task, responsibility, applicable checks, and expected result. Reviewers inspect and verify without repairing the change; remind them that other nodes may execute concurrently. Keep Git commands out of reviewer prompts; use files and observable outputs. Pass task and feedback through graph inputs rather than repeating them in long prompts. Resolve material ambiguities during the interview because runtime agents are non-interactive.

Leave optional deadlines, budgets, and retry restrictions unset unless requested. Do not copy small loop caps from examples. When the graph format requires finite bounds, derive permissive valid values from the current structural ceilings and the proposed graph's execution counts. Explain those required bounds briefly; preserve mandatory protocol limits and never claim the runtime is unbounded.

## Show the proposal

Briefly explain the repository evidence behind the verification choices. Then draw the graph in an inline ASCII code block, using only node titles or purposes, forks/joins, and high-level conditions. Omit schemas, field bindings, runtime details, and plumbing from the diagram.

For example, when two independent reviews are useful:

```text
[Implement / repair]
        |
        +----> [Review code] -----+
        |                        |
        +----> [Verify behavior] -+
                                 |
                       all accept? -- yes --> [Done]
                            |
                            no
                            +----> [Implement / repair]
```

Immediately below the diagram, show the complete authored instructions for every agent node, each in a labeled fenced text block. Quote them verbatim, including any shared authored instruction text. Do not summarize, elide, or hide instructions behind references. Explain variable inputs outside the quoted text. Structural nodes need no invented prompts.

Invite feedback on the graph and wording. After revisions, show the updated diagram and all current node instructions together so the user can review a complete version. Keep simplifying as you iterate. Wait for the user's acceptance; silence is not acceptance.

## Choose runtimes after acceptance

Only after the user is happy with both graph and instructions, ask:

> Which harness, model, provider, and effort would you like? You can use one configuration for all agent nodes or specify them per node. Effort can also stay at the harness default.

Reuse choices already supplied. Confirm the provider when needed to form a valid harness/provider lane. Keep model identifiers exactly as authored; do not infer providers from model names or invent a model catalog. Resolve incomplete choices before producing a runnable runtime plan.

Accept ordinary model names and resolve their technical IDs from existing configuration or official provider documentation. If the provider is unclear, ask only for the provider; do not ask the user for a model ID.

Prefer a continuing worker session and fresh independent reviewer sessions when supported, unless the user chooses otherwise. If runtime constraints require changing the graph or instructions, show the revision and get agreement on it before finalizing.

## Prepare the profile

Use installed Zeroshot help and version-matched documentation as the contract. The official [review-loop guide](https://the-open-engine.github.io/zeroshot/current/guides/review-loop/) and [runtime-plan reference](https://the-open-engine.github.io/zeroshot/current/reference/runtime-plan/) describe the separate graph and runtime documents; check compatibility with the installed version.

Produce `graph.json` and `runtime.json` in the user's chosen location, or a dedicated local artifact directory if none was specified. Preserve the reviewed node instructions verbatim. Bind every executable node and connect reviewer verdicts and diagnostics to the loop's acceptance condition and repair feedback. Give bound exhaustion and execution failures explicit unsuccessful outcomes.

Use supported validation that does not start a run, subject to repository guidance. Report what was actually validated; JSON parsing alone does not prove graph admission. If Zeroshot or full validation is unavailable, still provide the design and files and state the remaining validation clearly.

## Final step: save, configure, and explain how to run

Suggest a name and offer local, platform user, or platform organization storage. Use `cloud` unless another target is specified.

Before asking for login or an organization, check existing access with `zeroshot profile list --target TARGET --scope SCOPE`. Reuse the session and selected organization; ask only if unresolved. Follow host credential-store guidance if login is needed.

Check name conflicts, then confirm the name, destination, and any replacement once. An explicit save instruction counts as confirmation. Save with `zeroshot profile set NAME --graph graph.json --runtime-config runtime.json`, adding `--target TARGET --scope SCOPE` for platform storage. Read back and verify the saved profile; retain the draft and report any failure.

Ask whether the saved profile should become the default, naming the affected scope. If yes, use `zeroshot profile default NAME` with the matching target/scope flags. Reuse an earlier default decision.

Inspect the effective runtime's required environment/connection fields and existing configuration. If values are missing, ask how the user wants to provide them:

- Printed CLI setup commands: use `zeroshot connection set KEY --field FIELD` with repeated hidden prompts and the correct target/scope. Include every connection field because `set` replaces the connection.
- Platform UI: link to `https://app.zeroshot.sh/orgs/ORG_ID/connections?scope=SCOPE` using verified values, or link to [Platform](https://app.zeroshot.sh) and direct the user to Connections.
- At run startup: print a command that prompts without echo and supplies the required variables only to that run.

Never request secret values in chat or embed them in printed commands. Skip this question when no values are missing.

Finish with a copyable `zeroshot run` command containing the saved profile selector, title, input file, and target where applicable, plus minimal input JSON matching the graph. Include startup prompts if chosen. Do not start the run unless explicitly requested.
