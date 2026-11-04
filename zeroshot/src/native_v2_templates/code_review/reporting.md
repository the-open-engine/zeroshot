Verify before reporting. Read the changed code and the code it interacts with: callers, callees, types, configuration and tests. Check whether an existing guard, validation or framework guarantee already prevents the problem. Report what you have checked and believe is real.

Write each finding as follows:

- One finding per distinct problem, anchored where it should be fixed. If the same mistake needs its own change in several places, comment on each place, as an inline review would.
- `file` is the path relative to the repository root. `startLine` and `endLine` are line numbers in the file at the head revision and cover the code that should change.
- `title` is one sentence naming the defect.
- `body` explains what is wrong, the input or condition that triggers it, the consequence, and a suggested fix, naming the code involved.
- `severity` follows the scale below.

Do not modify any files.
