# How to configure _pydoclint_

This document shows you how to configure _pydoclint_.

## 1. All configuration options

For detailed explanations of all options, please read this page:
[Configuration options of _pydoclint_](https://jsh9.github.io/pydoclint/config_options.html).

## 2. Specifying options inline

In either your terminal window or your CI/CD file (such as `tox.ini`):

- Native _pydoclint_:

  ```bash
  pydoclint --check-arg-order=False <FILE_OR_FOLDER_PATH>
  ```

- Via Flake8:

  ```bash
  flake8 --check-arg-order=False <FILE_OR_FOLDER_PATH>
  ```

(Note: the `=` sign is not necessary but it is encouraged, because it's just a
bit more "Pythonic" and easier to read.)

## 3. Specifying options in a configuration file

- Native:

  - In a `.toml` file somewhere in your project folder, add a section like this
    (put in the config that you need):

    ```toml
    [tool.pydoclint]
    style = 'google'
    exclude = '\.git|\.tox|tests/data|some_script\.py'
    require-return-section-when-returning-nothing = true
    ```

  - Then, specify the path of the `.toml` file in your command:

    ```bash
    pydoclint --config=path/to/my/config.toml <FILE_OR_FOLDER_PATH>
    ```

  - Note: the default value for `--config` option is `pyproject.toml` (in the
    root directory of your project) if you do not specify the config file name.

- Flake8:

  - In your flake8 config file (see
    [flake8's official doc](https://flake8.pycqa.org/en/latest/user/configuration.html#configuration-locations)),
    add the config you need under the section `[flake8]`

## 4. Specifying options in `.pre-commit-config.yaml`

This is a template for `.pre-commit-config.yaml`:

<table>
<tr><th>As a native tool</th><th>As a flake8 plugin</th></tr>
<tr><td>

```yaml
- repo: https://github.com/jsh9/pydoclint
  rev: <latest_tag>
  hooks:
    - id: pydoclint
      args:
        - --style=google
        - --check-return-types=False
```

</td><td>

```yaml
- repo: https://github.com/pycqa/flake8
  rev: <latest_tag>
  hooks:
    - id: flake8
      additional_dependencies:
        - pydoclint==<latest_tag>
      args:
        - --style=google
        - --check-return-types=False
```

</td></tr>
</table>

If you've already specified all the configuration options in a config file,
here is how to cite them in `.pre-commit-config.yaml`:

<table>
<tr><th>As a native tool</th><th>As a flake8 plugin</th></tr>
<tr><td>

```yaml
- repo: https://github.com/jsh9/pydoclint
  rev: <latest_tag>
  hooks:
    - id: pydoclint
      args:
        - --config=pyproject.toml
```

</td><td>

```yaml
- repo: https://github.com/pycqa/flake8
  rev: <latest_tag>
  hooks:
    - id: flake8
      additional_dependencies:
        - Flake8-pyproject>=1.2.0
        - pydoclint==<latest_tag>
      args:
        - --toml-config=pyproject.toml
        - --style=google
        - --check-return-types=False
```

</td></tr>
</table>

Here is an example accompanying `pyproject.toml` config file:

<table>
<tr><th>As a native tool</th><th>As a flake8 plugin</th></tr>
<tr><td>

```toml
[tool.pydoclint]
check-return-types = false
style = "google"
```

</td><td>

```toml
[tool.flake8]
check-return-types = false
style = "google"
```

</td></tr>
</table>
