# Style Deviations

_pydoclint_ supports three mainstream docstring styles:
[numpy](https://numpydoc.readthedocs.io/en/latest/format.html),
[Google](https://www.sphinx-doc.org/en/master/usage/extensions/example_google.html),
and
[Sphinx](https://sphinx-rtd-tutorial.readthedocs.io/en/latest/docstrings.html).
But there are some minor deviations from the standard style for practical
reasons (such as unambiguity and speed).

**Table of Contents**

<!--TOC-->

- [1. numpy](#1-numpy)
  - [1.1. Type annotation](#11-type-annotation)
  - [1.2. Default values](#12-default-values)
- [2. Google](#2-google)
  - [2.1. Type annotation](#21-type-annotation)
  - [2.2. Default values](#22-default-values)
- [3. Sphinx](#3-sphinx)
  - [3.1. Yield type](#31-yield-type)

<!--TOC-->

## 1. numpy

### 1.1. Type annotation

These styles specified in the
[numpydoc website](https://numpydoc.readthedocs.io/en/latest/format.html#parameters)
are ***not*** accepted by _pydoclint_:

```
Parameters
----------
iterable : iterable object
shape : int or tuple of int
files : list of str
flag : int, optional
```

This is because these natural languages are inherently ambiguous, difficult to
check, and difficult to keep consistent within a project.

Instead, _pydoclint_ only accepts type hints that look identical to the
function signature:

```
Parameters
----------
iterable : Iterable[int]
shape : int | tuple[int, ...]
files : list[str]
flag : int | None, default=None
```

### 1.2. Default values

These styles to specify default values are ***not*** accepted by _pydoclint_:

```
flag : int, optional
something: int, default 2
onething: bool, default: False
```

These are accepted:

```
flag : int | None, default=None
something: int, default=2
onething: bool, default=False
```

This would ensure that different code maintainers would write consistent style
within the same project.

## 2. Google

### 2.1. Type annotation

By default, _pydoclint_ checks return type and yield type consistencies, and it
also requires argument types in the docstring. In other words, by _pydoclint_'s
default, this is an acceptable Google-style docstring:

```python
"""
This is a function.

Args:
    arg1 (int): Arg 1
    arg2 (float): Arg 2
    arg3 (Optional[Union[float, int, str]]): Arg 3

Returns:
    int: Result
"""
```

However, this may not be the convention of a lot of Google-style docstring
writers.

But do not worry: here are some config options to tweak:

- `--arg-type-hints-in-docstring`: you can set it to `False`
- `--check-return-types`: you can set it to `False`
- `--check-yield-types`: you can set it to `False`

[Here](https://jsh9.github.io/pydoclint/config_options.html) are all the
configurable options of _pydoclint_, and
[here](https://jsh9.github.io/pydoclint/how_to_config.html) is how to configure
_pydoclint_.

### 2.2. Default values

These styles to specify default values are ***not*** accepted by _pydoclint_:

```
flag (int, optional): The flag. Defaults to 1
```

This is accepted:

```
flag (int, default=None): The flag
```

## 3. Sphinx

### 3.1. Yield type

The
[official Sphinx documentation](https://sphinx-rtd-tutorial.readthedocs.io/en/latest/docstrings.html)
does not explicitly state this, so it is unclear what header to use to specify
the type of what a function yields.

Many people use `rtype`, but the authors of _pydoclint_ find it difficult to
differentiate the type of return value and the type of yield value.

Therefore, _pydoclint_ expects the convention of `ytype` for yield types. This
is actually common practice, as evident from a code search on GitHub:
https://github.com/search?q=%3Aytype%3A+language%3APython&type=code&l=Python
