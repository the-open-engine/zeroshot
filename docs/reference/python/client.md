---
description: "Python API reference for Client, Run and Stack, covering run and hosted stack submission, lookup, observation and stop operations."
---

# Client and run handle

::: zeroshot
    options:
      members:
        - Client
        - Run
        - Stack
      show_if_no_docstring: false

`await run.steer(text)` returns a `RunSteer` receipt for standing direction. It follows the handle's
local or named-target route, leaves suspended runs suspended and uses the existing typed errors.
Read complete text through `Run.vector(receipt.id)`.
