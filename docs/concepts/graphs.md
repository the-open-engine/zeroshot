---
description: 'The parts a zeroshot graph can be built from, from agent and script steps to people, timed waits, parallel groups and loops, with their current limits.'
---

# What a graph is made of

A graph is a typed program that zeroshot enforces. The built-in templates (`software-change`,
`pr-review`, `code-review`) are examples built from these parts.

| Building block                                       | What it is for                                            | Defined in                                                                    |
| ---------------------------------------------------- | --------------------------------------------------------- | ----------------------------------------------------------------------------- |
| Agent steps                                          | Let an agent do work with a typed input and output        | [Graph contract](../reference/cluster/graph.md)                               |
| Script steps                                         | Run a local executable with JSON in and out               | [Runtime plan](../reference/runtime-plan.md#script)                           |
| Script verifiers that emit declared signals          | Let a program you own decide a signal the graph routes on | [Graph contract](../reference/cluster/graph.md)                               |
| Ask a person (`builtin.request.ask@1`)               | Suspend until a person answers                            | [Ask a person](../guides/ask-a-person.md)                                     |
| Wait (`builtin.request.wait@1`)                      | Pause until a time or for a duration                      | [Wait until a time](../guides/wait.md)                                        |
| Parallel groups with `all`, `any`, `quorum`, `first` | Run branches side by side and join them by a rule         | [Graph contract](../reference/cluster/graph.md)                               |
| Map over a list of items                             | Run one body for each item of a list                      | [Graph contract](../reference/cluster/graph.md)                               |
| Bounded loops                                        | Repeat a body until a condition holds or the limit is hit | [Graph contract](../reference/cluster/graph.md)                               |
| Saved profiles                                       | Keep a graph with its runtime settings and reuse it       | [Saved-profile imports](../reference/target-runtime.md#saved-profile-imports) |
| File workflows                                       | Take files in and return a folder of results              | [Work with files and folders](../guides/file-workflows.md)                    |
| Stacks                                               | Submit dependent runs to a hosted target                  | [Submit a stack](../guides/stacks.md)                                         |
| Active Memory                                        | Carry lessons from earlier runs into later ones           | [Active Memory](../guides/active-memory.md)                                   |
| Python SDK                                           | Start and follow runs from Python                         | [Python SDK](../guides/python-sdk.md)                                         |
| ACP                                                  | Serve a local graph as an Agent Client Protocol agent     | [Use a local graph as an ACP agent](../guides/acp.md)                         |
| The plugin (MCP)                                     | Use zeroshot from ChatGPT, Codex or Claude                | [Install the zeroshot plugin](../guides/plugins.md)                           |

## Current limits

- Script bindings are local only. Named targets and contained placements reject them
  ([runtime plan](../reference/runtime-plan.md#script)).
- A parallel or mapped group has one checkpoint for the whole outer group
  ([Wait until a time](../guides/wait.md#long-work-over-many-items)).
- Ask and wait nodes are refused below `par` or `map`, and in stack profiles.
- Waits and request deadlines are capped at 30 days.
- A suspended run continues when a person answers, when you resume it or when a wait is due. Nothing
  else resumes it.
- Suspension requires workspace checkpoints.
