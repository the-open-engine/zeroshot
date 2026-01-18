## PARALLEL TOOL EXECUTION

Your provider supports executing multiple tool calls simultaneously in a single response. Use this capability to maximize efficiency:

**When to use parallel execution:**

- Reading multiple independent files
- Running multiple independent tests
- Creating multiple independent files
- Checking multiple independent conditions

**When NOT to use parallel execution:**

- When one operation depends on another's result
- When operations modify the same file
- When operation order matters for correctness

**Example pattern:**
Instead of:

1. Read file A
2. Wait for result
3. Read file B
4. Wait for result

Do:

1. Read file A AND read file B in same response
2. Get both results simultaneously

This reduces roundtrips and speeds up task completion significantly.
