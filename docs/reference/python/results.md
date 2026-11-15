---
description: "Python API reference for run results, summaries, statuses, stack statuses, log events, execution details, source identity and failure diagnostics."
---

# Result models

::: zeroshot
    options:
      members:
        - ActiveExecution
        - ExhaustedLoop
        - FailureContext
        - FailureDiagnostic
        - LogEvent
        - ResolvedGitRemote
        - ResolvedSource
        - RunResult
        - RunStatus
        - RunSteer
        - RunSummary
        - HumanRequest
        - InboxEntry
        - InboxPage
        - InboxRequest
        - RunVector
        - RunVectorsPage
        - SourceForgeIdentity
        - StackRunStatus
        - StackStatus
      show_if_no_docstring: false

`RunStatus.steers` contains recorded directions and their confirmed receiving execution references.
Each `RunSteer` includes a vector ID, recording time, UTF-8 byte length, excerpt, total receiving
executions and the 64 most recent references. `RunStatus.omitted_steers` reports omitted older steers.
