# How to ignore certain violations

## As a native tool

Currently, pydoclint does not support ignoring certain violations as a native
tool. Please use it as a _flake8_ plugin to achieve that, or feel free to
contribute this feature.

## As a _flake8_ plugin

In _flake8_ mode (meaning that you use _pydoclint_ as a flake8 plugin), if
you'd like to ignore a specific violation code (such as `DOC201` and `DOC301`)
in-line, you can add this comment to the function of your choice:

```python
def my_function(  # noqa: DOC201, DOC301
        arg1,
        arg2,
) -> None:
    ...
```

If you would like to ignore certain categories of violations (such as `DOC2xx`)
in-line, you can do this:

```python
def my_function(  # noqa: DOC2
        arg1,
        arg2,
) -> None:
    ...
```

All the usage is consistent with how you would use _flake8_. Please read the
official _flake8_ documentation for full details:
https://flake8.pycqa.org/en/latest/user/violations.html.

### Usage with [Ruff](https://github.com/astral-sh/ruff)

With `ruff>=0.1.3`, allowlist `DOC` codes using the
[`external` setting](https://docs.astral.sh/ruff/settings/#external):

Put the following in your `pyproject.toml` file:

```toml
[tool.ruff]
external = [
    "DOC",  # pydoclint
]
```
