# Notes for developers

**Table of Contents**

<!--TOC-->

- [1. Entry points](#1-entry-points)
- [2. How to quickly sanity-check a code example?](#2-how-to-quickly-sanity-check-a-code-example)
- [3. How to quickly debug a particular style violation?](#3-how-to-quickly-debug-a-particular-style-violation)
- [4. Why are names in _pydoclint_ in camelCase?](#4-why-are-names-in-pydoclint-in-camelcase)

<!--TOC-->

## 1. Entry points

The native entry point is in `pydoclint/main.py`.

The flake8 entry point is in `pydoclint/flake8_entry.py`.

Both entry points instantiates a `Visitor` object (in `pydoclint/visitor.py`).
All the checking happens in the `visit_***()` methods in the `Visitor` class.

## 2. How to quickly sanity-check a code example?

Put the code example in the file `tests/data/playground.py` (which is currently
empty). And then run the test `testPlayground()` (in
`tests/test_playground.py`). Adjust the options accordingly.

## 3. How to quickly debug a particular style violation?

For example, if you don't think the violation `DOC203` should be reported, but
_pydoclint_ reports `DOC203`, you can search for `v203` in `visitor.py` (all
violations are intentionally named this way in `visitor.py` for this purpose).
Find a suitable place where `v203` is appended to `violations`, and add your
breakpoint there.

## 4. Why are names in _pydoclint_ in camelCase?

Yes, this is a slightly unconventional style choice. The benefits of camelCase
are:

- Faster typing: `snake_cases_that_use_underscores` are slower to type than
  `camelCase`
- Space saving: using camelCase allows us to put more contents on the same line
  without line wrapping
