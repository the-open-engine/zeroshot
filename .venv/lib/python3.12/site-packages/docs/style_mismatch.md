# More about docstring style mismatch (`DOC003`)

This violation code warns you when _pydoclint_ thinks that the docstring is
written in a different style than the style you specified via the `--style`
config option.

## 1. How does _pydoclint_ detect the style of a docstring?

_pydoclint_ detects the style of a docstring with this procedure:

### 1.1. Numpy-style pattern detection (enhanced detection)

As of recent updates, _pydoclint_ first checks if the docstring contains
numpy-style section headers with dashes. If it detects patterns like:

```
Returns
-------

Parameters
----------

Examples
--------
```

It immediately identifies the docstring as numpy-style and parses it
accordingly, even if it may not be fully parsable as numpy style. This
pattern-based detection looks for common section headers (Args, Arguments,
Parameters, Returns, Yields, Raises, Examples, Notes, See Also, References)
followed by 3 or more dashes on the next line.

### 1.2. Fallback to size-based detection

If no numpy-style patterns are detected, _pydoclint_ falls back to the original
size-based detection:

- It attempts to parse the docstring in all 3 styles: numpy, Google, and Sphinx
- It then compares the "size" of the parsed docstring objects
  - The "size" is a human-made metric to measure how "fully parsed" a docstring
    object is. For example, a docstring object without the return section is
    larger in "size" than that with the return section (all others being equal)
- The style that yields the largest "size" is considered the style of the
  docstring

## 2. How accurate is this detection heuristic?

The authors of _pydoclint_ have manually tested this heuristic in 8
repositories written in all 3 styles (numpy, Google, and Sphinx), and have
found this heuristic to be satisfactory:

- Accuracy: 100%
- Precision: 100%
- Recall: 100%

However, we admit that 8 is too small a sample size to be statistically
representative. If you encounter any false positives or false negatives, please
don't hesitate to file an issue
[here](https://github.com/jsh9/pydoclint/issues).

## 3. Can I turn this off?

Actually, this style mismatch detection feature is by default _off_.

You can turn this feature on by setting `--check-style-mismatch` (or `-csm`) to
`True` (or `--check-style-mismatch=True`).

## 3. Is it much slower to parse a docstring in all 3 styles?

It is not. The authors of _pydoclint_ benchmarked some very large code bases,
and here are the results (as of 2025/01/12):

|                              | numpy | scikit-learn | Bokeh | Airflow |
| ---------------------------- | ----- | ------------ | ----- | ------- |
| Number of .py files          | 581   | 929          | 1196  | 5004    |
| Run time with 1 style [sec]  | 1.84  | 2.68         | 0.77  | 5.50    |
| Run time with 3 styles [sec] | 1.91  | 2.79         | 0.78  | 5.77    |
| Additional run time [sec]    | 0.07  | 0.11         | 0.01  | 0.07    |
| Relative additional run time | 4%    | 4%           | 1%    | 5%      |

## 5. What violation code is associated with style mismatch?

`DOC003`: "Docstring style mismatch".

## 6. How to fix this violation code?

You are suggested to check if the docstring style is consistent with what you
specified via the `--style` config option. If not, please rewrite your
docstring, or specify the correct style via `--style`.

Also, please note that specifying an incorrect docstring style may mask other
violations. So after you fix the docstring style, you may need to fix other
"new" (previously hidden) violations.
