# Configuration options of _pydoclint_

There are many configuration options available. They can be used invidually or
together.

For how to actually implement these options in your commands, please read this
page:
[How to configure _pydoclint_](https://jsh9.github.io/pydoclint/how_to_config.html).

**Table of Contents**

<!--TOC-->

- [1. `--quiet` (shortform: `-q`)](#1---quiet-shortform--q)
- [2. `--exclude`](#2---exclude)
- [3. `--style`](#3---style)
- [4. `--arg-type-hints-in-docstring` and `--arg-type-hints-in-signature`](#4---arg-type-hints-in-docstring-and---arg-type-hints-in-signature)
- [5. `--check-arg-order` (shortform: `-ao`, default: `True`)](#5---check-arg-order-shortform--ao-default-true)
- [6. `--skip-checking-short-docstrings` (shortform: `-scsd`, default: `True`)](#6---skip-checking-short-docstrings-shortform--scsd-default-true)
- [7. `--skip-checking-raises` (shortform: `-scr`, default: `False`)](#7---skip-checking-raises-shortform--scr-default-false)
- [8. `--allow-init-docstring` (shortform: `-aid`, default: `False`)](#8---allow-init-docstring-shortform--aid-default-false)
- [9. `--require-return-section-when-returning-nothing` (shortform: `-rrs`, default: `False`)](#9---require-return-section-when-returning-nothing-shortform--rrs-default-false)
- [10. `--check-return-types` (shortform: `-crt`, default: `True`)](#10---check-return-types-shortform--crt-default-true)
- [11. `--require-yield-section-when-yielding-nothing` (shortform: `-rys`, default: `True`)](#11---require-yield-section-when-yielding-nothing-shortform--rys-default-true)
- [12. `--check-yield-types` (shortform: `-cyt`, default: `True`)](#12---check-yield-types-shortform--cyt-default-true)
- [13. `--ignore-underscore-args` (shortform: `-iua`, default: `True`)](#13---ignore-underscore-args-shortform--iua-default-true)
- [14. `--ignore-private-args` (shortform: `-ipa`, default: `False`)](#14---ignore-private-args-shortform--ipa-default-false)
- [15. `--check-class-attributes` (shortform: `-cca`, default: `True`)](#15---check-class-attributes-shortform--cca-default-true)
- [16. `--should-document-private-class-attributes` (shortform: `-sdpca`, default: `False`)](#16---should-document-private-class-attributes-shortform--sdpca-default-false)
- [17. `--treat-property-methods-as-class-attributes` (shortform: `-tpmaca`, default: `False`)](#17---treat-property-methods-as-class-attributes-shortform--tpmaca-default-false)
- [18. `--only-attrs-with-ClassVar-are-treated-as-class-attrs` (shortform: `-oawcv`, default: `False`)](#18---only-attrs-with-classvar-are-treated-as-class-attrs-shortform--oawcv-default-false)
- [19. `--should-document-star-arguments` (shortform: `-sdsa`, default: `True`)](#19---should-document-star-arguments-shortform--sdsa-default-true)
- [20. `--check-style-mismatch` (shortform: `-csm`, default: `False`)](#20---check-style-mismatch-shortform--csm-default-false)
- [21. `--check-arg-defaults` (shortform: `-cad`, default: `False`)](#21---check-arg-defaults-shortform--cad-default-false)
- [22. `--baseline`](#22---baseline)
- [23. `--generate-baseline` (default: `False`)](#23---generate-baseline-default-false)
- [24. `--auto-regenerate-baseline` (shortform: `-arb`, default: `True`)](#24---auto-regenerate-baseline-shortform--arb-default-true)
- [25. `--show-filenames-in-every-violation-message` (shortform: `-sfn`, default: `False`)](#25---show-filenames-in-every-violation-message-shortform--sfn-default-false)
- [26. `--config` (default: `pyproject.toml`)](#26---config-default-pyprojecttoml)

<!--TOC-->

## 1. `--quiet` (shortform: `-q`)

This flag activates the "quite mode", in which no output will be printed to the
command line if there are no violations.

By default, this flag is _not_ activated, so the files that are scanned are
printed in the command line.

```
pydoclint --quiet <FILE_OR_FOLDER>
```

This option is only available in the "native" command-line mode, rather than in
flake8. If you use pydoclint in flake8, please use flake8's own verbosity
configuration instead.

## 2. `--exclude`

You can use this option to exclude files within the given folder. It is a regex
pattern of full file paths.

For example:

```
pydoclint --exclude='\.git|\.tox|tests/data' <FOLDER_NAME>
```

This option is only available in the native command-line mode. If you use
_pydoclint_ within _flake8_, you can use _flake8_'s
[`--exclude` option](https://flake8.pycqa.org/en/latest/user/options.html#cmdoption-flake8-exclude).

## 3. `--style`

Which style of docstring is your code base using. Right now there are three
available choices: `numpy`, `google`, and `sphinx`. The default value is
`numpy`.

```
pydoclint --style=google <FILE_OR_FOLDER>
```

or

```
flake8 --style=google <FILE_OR_FOLDER>
```

## 4. `--arg-type-hints-in-docstring` and `--arg-type-hints-in-signature`

- `--arg-type-hints-in-docstring`
  - Shortform: `-athd`
  - Default: `True`
  - Meaning:
    - If `True`, there need to be type hints in the argument list of a
      docstring
    - If `False`, there cannot be any type hints in the argument list of a
      docstring
- `--arg-type-hints-in-signature`
  - Shortform: `-aths`
  - Default: `True`
  - Meaning:
    - If `True`, there need to be type hints for input arguments in the
      function/method signature
    - If `False`, there cannot be any type hints for input arguments in the
      function/method signature

Note: if users choose `True` for both options, the argument type hints in the
signature and in the docstring need to match, otherwise there will be a style
violation.

## 5. `--check-arg-order` (shortform: `-ao`, default: `True`)

If `True`, the input argument order in the docstring needs to match that in the
function signature.

To turn this option on/off, do this:

```
pydoclint --check-arg-order=False <FILE_OR_FOLDER>
```

or

```
flake8 --check-arg-order=False <FILE_OR_FOLDER>
```

## 6. `--skip-checking-short-docstrings` (shortform: `-scsd`, default: `True`)

If `True`, `pydoclint` won't check functions that have only a short description
in their docstring.

To turn this option on/off, do this:

```
pydoclint --skip-checking-short-docstrings=False <FILE_OR_FOLDER>
```

or

```
flake8 --skip-checking-short-docstrings=False <FILE_OR_FOLDER>
```

## 7. `--skip-checking-raises` (shortform: `-scr`, default: `False`)

If `True`, _pydoclint_ won't report `DOC501` or `DOC502` if there are `raise`
statements in the function/method but there aren't any "raises" sections in the
docstring (or vice versa).

## 8. `--allow-init-docstring` (shortform: `-aid`, default: `False`)

If it is set to `True`, having a docstring for class constructors
(`__init__()`) is allowed, and the arguments are expected to be documented
under `__init__()` rather than in the class docstring.

Note: the default is set to `False` because not every class has an `__init__`
method (such as classes that inherit from parent classes), but every class must
have the `class ClassName` declaration.

## 9. `--require-return-section-when-returning-nothing` (shortform: `-rrs`, default: `False`)

If `False`, a "return" section is not necessary in the docstring if the
function implicitly returns `None` (for example, doesn't have a return
statement, or has `-> None` as the return annotation) or doesn't return at all
(has return type `NoReturn`).

## 10. `--check-return-types` (shortform: `-crt`, default: `True`)

If True, check that the type(s) in the docstring return section and the return
annotation in the function signature are consistent

## 11. `--require-yield-section-when-yielding-nothing` (shortform: `-rys`, default: `True`)

If False, a yields section is not needed in docstring if the function yields
None.

## 12. `--check-yield-types` (shortform: `-cyt`, default: `True`)

If True, check that the type(s) in the docstring "yields" section and the
return annotation in the function signature are consistent.

## 13. `--ignore-underscore-args` (shortform: `-iua`, default: `True`)

If True, underscore arguments (such as \_, \_\_, ...) in the function signature
do not need to appear in the docstring.

Note: "underscore arguments" are not the same as "private arguments" (i.e.,
"arguments with leading underscores") such as `_a`.

## 14. `--ignore-private-args` (shortform: `-ipa`, default: `False`)

If True, private arguments (those with leading underscores in their names but
are not purely `_`, `__`, etc.) in the function signature do not need to appear
in the docstring.

## 15. `--check-class-attributes` (shortform: `-cca`, default: `True`)

If True, check the class attributes (defined under the class definition)
against the "Attributes" section of the class's docstring.

Please read
[this page](https://jsh9.github.io/pydoclint/checking_class_attributes.html)
for more instructions.

## 16. `--should-document-private-class-attributes` (shortform: `-sdpca`, default: `False`)

If True, private class attributes (those that start with leading `_`) should be
documented. If False, they should not be documented.

## 17. `--treat-property-methods-as-class-attributes` (shortform: `-tpmaca`, default: `False`)

If True, treat `@property` methods as class properties. This means that they
need to be documented in the "Attributes" section of the class docstring, and
there cannot be any docstring under the @property methods. This option is only
effective when --check-class-attributes is True.

## 18. `--only-attrs-with-ClassVar-are-treated-as-class-attrs` (shortform: `-oawcv`, default: `False`)

If True, only the attributes whose type annotations are wrapped within
`ClassVar` (where `ClassVar` is imported from `typing`) are treated as class
attributes, and all other attributes are treated as instance attributes.

## 19. `--should-document-star-arguments` (shortform: `-sdsa`, default: `True`)

If True, "star arguments" (such as `*args`, `**kwargs`, `**props`, etc.) in the
function signature should be documented in the docstring. If False, they should
not appear in the docstring.

## 20. `--check-style-mismatch` (shortform: `-csm`, default: `False`)

If True, check that style specified in --style matches the detected style of
the docstring. If there is a mismatch, DOC003 will be reported. Setting this to
False will silence all DOC003 violations.

## 21. `--check-arg-defaults` (shortform: `-cad`, default: `False`)

If True, docstring type hints should contain default values consistent with the
function signature. If False, docstring type hints should not contain default
values. (Only applies to numpy style for now.)

## 22. `--baseline`

Baseline allows you to remember the current project state and then show only
new violations, ignoring old ones. This can be very useful when you'd like to
gradually adopt _pydoclint_ in existing projects.

If you'd like to use this feature, pass in the full file path to this option.
For convenience, you can write this option in your `pyproject.toml` file:

```toml
[tool.pydoclint]
baseline = "pydoclint-baseline.txt"
```

If you also set `--generate-baseline=True` (or `--generate-baseline True`),
_pydoclint_ will generate a file that contains all current violations of your
project.

If `--generate-baseline` is not passed to _pydoclint_ (the default is `False`),
_pydoclint_ will read your baseline file, and ignore all violations specified
in that file.

## 23. `--generate-baseline` (default: `False`)

Required to use with `--baseline` option. If `True`, generate the baseline file
that contains all current violations.

## 24. `--auto-regenerate-baseline` (shortform: `-arb`, default: `True`)

If it's set to True, _pydoclint_ will automatically regenerate the baseline
file every time you fix violations in the baseline and rerun _pydoclint_.

This saves you from having to manually regenerate the baseline file by setting
`--generate-baseline=True` and run _pydoclint_.

## 25. `--show-filenames-in-every-violation-message` (shortform: `-sfn`, default: `False`)

If False, in the terminal the violation messages are grouped by file names:

```
file_01.py
    10: DOC101: ...
    25: DOC105: ...
    37: DOC203: ...

file_02.py
    24: DOC102: ...
    51: DOC107: ...
    126: DOC203: ...
    246: DOC105: ...
```

If True, the file names are printed in the front of every violation message:

```
file_01.py:10: DOC101: ...
file_01.py:25: DOC105: ...
file_01.py:37: DOC203: ...

file_02.py:24: DOC102: ...
file_02.py:51: DOC107: ...
file_02.py:126: DOC203: ...
file_02.py:246: DOC105: ...
```

This can be convenient if you would like to click on each violation message and
go to the corresponding line in your IDE. (Note: not all terminal app offers
this functionality.)

## 26. `--config` (default: `pyproject.toml`)

The full path of the .toml config file that contains the config options. Note
that the command line options take precedence over the .toml file. Look at this
page:
[How to configure _pydoclint_](https://jsh9.github.io/pydoclint/how_to_config.html)
