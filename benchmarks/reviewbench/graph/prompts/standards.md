{{intro}}

Review thoroughly. Start with the riskiest changes: trace each changed behavior from its inputs to its effects, through its callers and edge cases. Then go through every changed file and hunk in your scope, including tests, documentation and configuration, and ask what can go wrong, which inputs break it, and what else should have changed with it. Report every real problem you find, serious or minor; the severity says how much it matters.

Look for:
- Correctness: wrong logic, inverted or incomplete conditions, off-by-one errors, wrong variables, unhandled edge cases (empty, missing, zero, very large or unusual input), incorrect use of APIs, and wrong units, types or defaults.
- Consistency across the codebase: callers, implementations, configuration, schemas, migrations, serialization, routes, or documentation that no longer agree with the changed code, and behavior changes that break existing users.
- Error handling and reliability: swallowed or misreported errors, missing cleanup, resource leaks, races, ordering and concurrency problems, and partial failures that leave bad state.
- Security: vulnerabilities an attacker can reach, such as injection, missing authorization, exposed secrets, unsafe deserialization or path traversal. Name the input the attacker controls and how it reaches the code; hardening advice without such a path is not a finding.
- Performance: clear regressions on realistic input, such as repeated or quadratic work, queries in loops, or unbounded memory use.
- Interfaces: public functions, types, endpoints and configuration that are inconsistent with similar ones in this codebase, easy to misuse, or changed in ways that break existing users.
- Tests: tests that do not exercise what they claim, assertions that cannot fail, and specific test cases missing for new behavior or its edge cases.
- Documentation and comments: statements that are wrong or stale given the code, wrong file paths, commands, option names or examples, and typos in identifiers, messages and other user-facing text.
- Maintainability: misleading names, dead or unreachable code, leftover debugging code, duplicated logic that must be kept in step, and departures from conventions this codebase follows elsewhere.

If the change relies on existing code that is wrong, report that as well and say in the body that it predates the change.

Do not report pure formatting or personal style preferences, generic advice that is not tied to specific code here, or speculation you could not check.

{{reporting}}
