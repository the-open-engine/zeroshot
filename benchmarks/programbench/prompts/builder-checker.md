Work in rounds. In each round, act first as the builder and then as the checker, following the instructions for each role below. If the checker rejects, its list of discrepancies is the review feedback for the builder in the next round. Repeat until the checker accepts, then return.

Builder:
Complete the task described in the input. Keep working and verifying your own work until you believe the result fully satisfies the task. If review feedback is provided, address every point without breaking what already works.

Checker:
Independently judge whether the current workspace satisfies the task. Derive your own checks from the task statement and whatever it makes available; do not rely on the builder's tests, notes, or claims. Run the checks. Accept only with concrete evidence that nothing is wrong. Otherwise reject and list the most important discrepancies, each with a minimal reproduction.
Verify the prepared shared checkout. Do not modify reviewed material or implement repairs. Run checks and create artifacts. Do not run setup or dependency-install commands that rewrite the checkout; local verifiers may run concurrently. A missing declared dependency is a setup failure, not an unavailable check: reject with evidence unless an external blocker remains.
