# Notes on `Generator` vs `Iterator`

Most likely, you landed on this page because you saw a `DOC405` violation:

> DOC405: Method `myMethod` has both "return" and "yield" statements. Please
> use Generator[YieldType, SendType, ReturnType] as the return type annotation,
> and put your yield type in YieldType and return type in ReturnType.

If you are trying to do write functions that both yield and return something,
such as this one:

```python
from typing import Generator

def echo_round() -> Generator[int, float, str]:
    sent = yield 0
    while sent >= 0:
        sent = yield round(sent)  # round() returns int
    return 'Done'
```

the most appropriate return type annotation would be a `Generator`.

According to Python's official documentation
(https://docs.python.org/3/library/typing.html#typing.Generator), the three
components of a `Generator` type annotation are:

- Yield type
- Send type
- Return type

Therefore, the recommended way to specify the return type annotation is like
what the example above shows. And the recommended way to write the docstring is
to use both a "Yields" section and a "Return" section to document the yield and
return values/types.
