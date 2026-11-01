Your working directory is the repository checked out at the head revision, and the diff of the change is at `diffPath`; `summary` describes the change. Each field ending in `Findings` holds one reviewer's findings: the `unitFindings` fields have one list per part of the change, the others cover the whole change.

{{environment}}

1. Merge duplicates: findings that describe the same defect, however they are worded or anchored, become one finding that keeps the clearest explanation and any useful detail from the others. Keep different defects separate, even on the same lines, and keep one finding for each place where a repeated mistake needs its own change.
2. Try to disprove each remaining finding, as a skeptic:
   - Reachability: can the triggering input or sequence of events actually occur through the real callers, entry points and configuration?
   - Guards: does an existing check, validation, type, lock, transaction, framework guarantee or caller already prevent the problem?
   - Context: does the claim hold against code the reviewer may not have read, such as other callers, overrides, configuration and tests?
   Remove a finding that one of these tests disproves, a matter of taste that no convention in this codebase supports, generic advice not tied to this code, and a security finding without an input the attacker controls and a route from it to the vulnerable code. A problem in existing code that the change relies on stays, marked as predating the change. Keep the other findings, minor ones included.
3. For each finding you keep, open the file and make `file`, `startLine` and `endLine` cover the code that should change at the head revision, and make `title` and `body` precise: the defect, the condition that triggers it, the consequence and a suggested fix. Set `severity` on this scale:

{{severity}}

Return the findings you keep, or an empty list. Do not modify any files.
