---
description: 'The parts a zeroshot graph can be built from, from agent and script steps to people, timed waits, parallel groups and loops, and the ways to run and reuse a graph.'
---

# What a graph is made of

A graph is a typed program that zeroshot enforces. The built-in templates (`software-change`,
`pr-review`, `code-review`) are examples built from these parts.

## Steps and structure

| Part                                               | What it does                                                            |
| -------------------------------------------------- | ----------------------------------------------------------------------- |
| [Agent step](../reference/cluster/graph.md)        | An agent does work with a typed input and output                        |
| [Script step](../reference/runtime-plan.md#script) | A local executable runs with JSON in and out                            |
| [Script verifier](../reference/cluster/graph.md)   | A program you own decides a signal the graph routes on                  |
| [Ask a person](../guides/ask-a-person.md)          | The run suspends until a person answers                                 |
| [Wait](../guides/wait.md)                          | The run pauses until a time or for a duration                           |
| [Parallel group](../reference/cluster/graph.md)    | Branches run side by side and join by `all`, `any`, `quorum` or `first` |
| [Map](../reference/cluster/graph.md)               | One body runs for each item of a list                                   |
| [Loop](../reference/cluster/graph.md)              | A body repeats until a condition holds or its limit is reached          |

## Running and reusing a graph

| Part                                                                  | What it does                                           |
| --------------------------------------------------------------------- | ------------------------------------------------------ |
| [Saved profile](../reference/target-runtime.md#saved-profile-imports) | Keeps a graph with its runtime settings for reuse      |
| [File workflow](../guides/file-workflows.md)                          | Takes files in and returns a folder of results         |
| [Stack](../guides/stacks.md)                                          | Submits dependent runs to a hosted target              |
| [Active Memory](../guides/active-memory.md)                           | Carries lessons from earlier runs into later ones      |
| [Python SDK](../guides/python-sdk.md)                                 | Starts and follows runs from Python                    |
| [ACP](../guides/acp.md)                                               | Serves a local graph as an Agent Client Protocol agent |
| [Plugin](../guides/plugins.md)                                        | Runs zeroshot from ChatGPT, Codex or Claude            |
