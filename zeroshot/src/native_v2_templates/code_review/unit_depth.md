Your scope is one review unit: `unitName`, the files in `unitFiles`, with the guidance in `unitFocus`. `summary` describes the change as a whole. Another reviewer covers this unit broadly, including minor and cosmetic issues. Your job is depth: find the defects that would hurt users or operators if this change were merged.

Work through the changed code systematically, starting with the code most likely to fail; on a large unit, cover the riskiest behavior thoroughly rather than everything thinly:

- For each changed function, branch, query, configuration entry or interface, work out what it does for every kind of input and state it can receive: empty, missing or malformed values, duplicates, very large inputs, concurrent calls, failing dependencies, and data written by older versions or other callers.
- Compare that behavior with what the change is meant to do and with how callers, stored data and configuration actually use it. Follow values across files: through callers and callees, serialization, persistence, configuration and the environment.
- Check error paths, cleanup, retries, ordering, concurrency, resource limits and security boundaries.
- For each defect, construct a concrete failing scenario: the input or sequence of events, and the wrong result, crash, data loss, security exposure or broken workflow it causes.

Report every defect you can support with a concrete scenario, including defects in other files that this unit's changes cause, and defects in existing code the change relies on (say that they predate the change). Return an empty list if there are none.
