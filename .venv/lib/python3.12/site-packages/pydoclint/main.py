from __future__ import annotations

import ast
import logging
import re
from pathlib import Path

import click

from pydoclint import __version__
from pydoclint.baseline import (
    generateBaseline,
    parseBaseline,
    reEvaluateBaseline,
)
from pydoclint.parse_config import (
    injectDefaultOptionsFromUserSpecifiedTomlFilePath,
)
from pydoclint.utils.invisibleChars import replaceInvisibleChars
from pydoclint.utils.violation import Violation
from pydoclint.visitor import Visitor

# Due to a potential bug in Windows + pre-commit, non-ASCII
# characters cannot be rendered correctly as stdout in the terminal.
# Therefore, we set all CLI output as stderr.
# (More details in https://github.com/jsh9/pydoclint/issues/20)
echoAsError = True


def validateStyleValue(
        context: click.Context,
        param: click.Parameter,
        value: str | None,
) -> str | None:
    """Validate the value of the 'style' option"""
    if value not in {'numpy', 'google', 'sphinx'}:
        raise click.BadParameter(
            '"--style" must be "numpy", "google", or "sphinx"'
        )

    return value


@click.command(
    context_settings={'help_option_names': ['-h', '--help']},
    help='Pydoclint, a linter for Python docstring styles',
)
@click.option(
    '-q',
    '--quiet',
    is_flag=True,
    default=False,
    help='If True, do not print the file names being checked to the terminal.',
)
@click.option(
    '--exclude',
    type=str,
    show_default=True,
    default=r'\.git|\.tox',
    help=(
        'Regex pattern to exclude files/folders. Please add quotes (both'
        ' double and single quotes are fine) around the regex in the'
        ' command line.'
    ),
)
@click.option(
    '--style',
    type=str,
    show_default=True,
    default='numpy',
    callback=validateStyleValue,
    help='',
)
@click.option(
    '-ths',
    '--type-hints-in-signature',
    show_default=True,
    default='None',
    help='(Deprecated) Please use --arg-type-hints-in-signature instead',
)
@click.option(
    '-aths',
    '--arg-type-hints-in-signature',
    type=bool,
    show_default=True,
    default=True,
    help='Whether to require argument type hints in function signatures',
)
@click.option(
    '-thd',
    '--type-hints-in-docstring',
    show_default=True,
    default='None',
    help='(Deprecated) Please use --arg-type-hints-in-docstring instead',
)
@click.option(
    '-athd',
    '--arg-type-hints-in-docstring',
    type=bool,
    show_default=True,
    default=True,
    help='Whether to require type hints in the argument list in docstrings',
)
@click.option(
    '-ao',
    '--check-arg-order',
    type=bool,
    show_default=True,
    default=True,
    help='Whether to check docstring argument order against function signature',
)
@click.option(
    '-scsd',
    '--skip-checking-short-docstrings',
    type=bool,
    show_default=True,
    default=True,
    help='If True, skip checking if the docstring only has a short summary.',
)
@click.option(
    '-scr',
    '--skip-checking-raises',
    type=bool,
    show_default=True,
    default=False,
    help='If True, skip checking docstring "Raises" section against "raise" statements',
)
@click.option(
    '-aid',
    '--allow-init-docstring',
    type=bool,
    show_default=True,
    default=False,
    help='If True, allow both __init__() and the class def to have docstrings',
)
@click.option(
    '--require-return-section-when-returning-none',
    show_default=True,
    default='None',
    help=(
        '(Deprecated) Please use'
        ' --require-return-section-when-returning-nothing instead.'
    ),
)
@click.option(
    '-rrs',
    '--require-return-section-when-returning-nothing',
    type=bool,
    show_default=True,
    default=False,
    help=(
        'If False, a return section is not needed in docstring if'
        ' the function body does not have a "return" statement and'
        ' the return type annotation is "-> None" or "-> NoReturn".'
    ),
)
@click.option(
    '-crt',
    '--check-return-types',
    type=bool,
    show_default=True,
    default=True,
    help=(
        'If True, check that the type(s) in the docstring return section and'
        ' the return annotation in the function signature are consistent'
    ),
)
@click.option(
    '-rys',
    '--require-yield-section-when-yielding-nothing',
    type=bool,
    show_default=True,
    default=False,
    help=(
        'If False, a yields section is not needed in docstring if'
        ' the function yields None.'
    ),
)
@click.option(
    '-cyt',
    '--check-yield-types',
    type=bool,
    show_default=True,
    default=True,
    help=(
        'If True, check that the type(s) in the docstring "yields" section and'
        ' the return annotation in the function signature are consistent'
    ),
)
@click.option(
    '-iua',
    '--ignore-underscore-args',
    type=bool,
    show_default=True,
    default=True,
    help=(
        'If True, underscore arguments (such as _, __, ...) in the function'
        ' signature do not need to appear in the docstring. Note: "underscore'
        ' arguments" are not the same as "arguments with leading'
        ' underscores" (such as `_a`).'
    ),
)
@click.option(
    '-ipa',
    '--ignore-private-args',
    type=bool,
    show_default=True,
    default=False,
    help=(
        'If True, private arguments (those with leading underscores in their'
        ' names but are not purely `_`, `__`, etc.) in the function signature'
        ' do not need to appear in the docstring.'
    ),
)
@click.option(
    '-cca',
    '--check-class-attributes',
    type=bool,
    show_default=True,
    default=True,
    help=(
        'If True, class attributes (the ones defined right beneath'
        ' "class MyClass:") are checked against the docstring.'
    ),
)
@click.option(
    '-sdpca',
    '--should-document-private-class-attributes',
    type=bool,
    show_default=True,
    default=False,
    help=(
        'If True, private class attributes (the ones starting with _)'
        ' should be documented in the docstring. If False, private'
        ' class attributes should not appear in the docstring.'
    ),
)
@click.option(
    '-tpmaca',
    '--treat-property-methods-as-class-attributes',
    type=bool,
    show_default=True,
    default=False,
    help=(
        'If True, treat @property methods as class properties. This means'
        ' that they need to be documented in the "Attributes" section of'
        ' the class docstring, and there cannot be any docstring under'
        ' the @property methods. This option is only effective when'
        ' --check-class-attributes is True. We recommend setting both'
        ' this option and --check-class-attributes to True.'
    ),
)
@click.option(
    '-oawcv',
    '--only-attrs-with-ClassVar-are-treated-as-class-attrs',
    type=bool,
    show_default=True,
    default=False,
    help=(
        'If True, only the attributes whose type annotations are wrapped'
        ' within `ClassVar` (where `ClassVar` is imported from `typing`)'
        ' are treated as class attributes, and all other attributes are'
        ' treated as instance attributes.'
    ),
)
@click.option(
    '-sdsa',
    '--should-document-star-arguments',
    type=bool,
    show_default=True,
    default=True,
    help=(
        'If True, "star arguments" (such as *args, **kwargs,'
        ' **props, etc.) in the function signature should be'
        ' documented in the docstring. If False, they should not'
        ' appear in the docstring.'
    ),
)
@click.option(
    '-sdae',
    '--should-declare-assert-error-if-assert-statement-exists',
    type=bool,
    show_default=True,
    default=False,
    help=(
        'If True, we need to declare AssertError in the "Raises"'
        ' section of the docstring if there are any "assert"'
        ' statements in the function body. This is because an "assert"'
        ' statement could raise an AssertError.'
    ),
)
@click.option(
    '-csm',
    '--check-style-mismatch',
    type=bool,
    show_default=True,
    default=False,
    help=(
        'If True, check that style specified in --style matches the detected'
        ' style of the docstring. If there is a mismatch, DOC003 will be'
        ' reported. Setting this to False will silence all DOC003 violations.'
    ),
)
@click.option(
    '-cad',
    '--check-arg-defaults',
    type=bool,
    show_default=True,
    default=False,
    help=(
        'If True, docstring type hints should contain default values '
        'consistent with the function signature. If False, docstring '
        'type hints should not contain default values. (Only applies '
        'to numpy style for now.)'
    ),
)
@click.option(
    '--baseline',
    type=click.Path(
        exists=False,
        file_okay=True,
        dir_okay=False,
        readable=True,
        writable=True,
        allow_dash=True,
    ),
    help=(
        'The file name containing the existing violations (the "baseline").'
        ' If specified, only new violations will be reported, and the'
        ' violations in the baseline file are ignored.'
    ),
)
@click.option(
    '--generate-baseline',
    type=bool,
    show_default=True,
    default=False,
    help=(
        'If True, generates a new baseline file. (The name of the baseline'
        ' file should be specified by the --baseline option.)'
    ),
)
@click.option(
    '-arb',
    '--auto-regenerate-baseline',
    type=bool,
    show_default=True,
    default=True,
    help=(
        'If True, automatically regenerate the baseline file every time'
        ' pydoclint runs successfully.'
    ),
)
@click.option(
    '-sfn',
    '--show-filenames-in-every-violation-message',
    type=bool,
    show_default=True,
    default=False,
    help='If True, show file names in the front of every violation message.',
)
@click.argument(
    'paths',
    nargs=-1,
    type=click.Path(
        exists=True,
        file_okay=True,
        dir_okay=True,
        readable=True,
        allow_dash=True,
    ),
    is_eager=True,
)
@click.option(
    '--config',
    type=click.Path(
        exists=False,
        file_okay=True,
        dir_okay=False,
        readable=True,
        allow_dash=False,
        path_type=str,
    ),
    is_eager=True,
    callback=injectDefaultOptionsFromUserSpecifiedTomlFilePath,
    default='pyproject.toml',
    help=(
        'The full path of the .toml config file that contains the config'
        ' options; note that the command line options take precedence'
        ' over the .toml file'
    ),
)
@click.version_option(__version__)
@click.pass_context
def main(  # noqa: C901
        ctx: click.Context,
        quiet: bool,
        exclude: str,
        style: str,
        paths: tuple[str, ...],
        type_hints_in_signature: str,
        type_hints_in_docstring: str,
        arg_type_hints_in_signature: bool,
        arg_type_hints_in_docstring: bool,
        check_arg_order: bool,
        skip_checking_short_docstrings: bool,
        skip_checking_raises: bool,
        allow_init_docstring: bool,
        check_return_types: bool,
        check_yield_types: bool,
        ignore_underscore_args: bool,
        ignore_private_args: bool,
        check_class_attributes: bool,
        should_document_private_class_attributes: bool,
        treat_property_methods_as_class_attributes: bool,
        require_return_section_when_returning_none: bool,
        require_return_section_when_returning_nothing: bool,
        require_yield_section_when_yielding_nothing: bool,
        only_attrs_with_classvar_are_treated_as_class_attrs: bool,
        should_document_star_arguments: bool,
        should_declare_assert_error_if_assert_statement_exists: bool,
        check_style_mismatch: bool,
        check_arg_defaults: bool,
        generate_baseline: bool,
        auto_regenerate_baseline: bool,
        baseline: str,
        show_filenames_in_every_violation_message: bool,
        config: str | None,  # don't remove it b/c it's required by `click`
) -> None:
    """Command-line entry point of pydoclint"""
    logging.basicConfig(level=logging.WARN if quiet else logging.INFO)
    ctx.ensure_object(dict)

    if type_hints_in_docstring != 'None':  # it means users supply this option
        click.echo(
            click.style(
                ''.join(
                    [
                        'The option `--type-hints-in-docstring` has been renamed;',
                        ' please use `--arg-type-hints-in-docstring` instead',
                    ]
                ),
                fg='red',
                bold=True,
            ),
            err=echoAsError,
        )
        ctx.exit(1)

    if type_hints_in_signature != 'None':  # it means users supply this option
        click.echo(
            click.style(
                ''.join(
                    [
                        'The option `--type-hints-in-signature` has been renamed;',
                        ' please use `--arg-type-hints-in-signature` instead',
                    ]
                ),
                fg='red',
                bold=True,
            ),
            err=echoAsError,
        )
        ctx.exit(1)

    # it means users supply this option
    if require_return_section_when_returning_none != 'None':  # type:ignore[comparison-overlap]
        click.echo(
            click.style(
                ''.join(
                    [
                        'The option `--require-return-section-when-returning-none`',
                        ' has been renamed; please use',
                        '`--require-return-section-when-returning-nothing` instead',
                    ]
                ),
                fg='red',
                bold=True,
            ),
            err=echoAsError,
        )
        ctx.exit(1)

    # it means users supply this option
    if baseline is not None:
        baselinePath = Path(baseline)
        if not (generate_baseline or baselinePath.exists()):
            click.echo(
                click.style(
                    "The baseline file was specified but it doesn't exist.\n"
                    'Use `--generate-baseline=True` to generate it first.',
                    fg='red',
                    bold=True,
                ),
                err=echoAsError,
            )
            ctx.exit(1)

    if len(paths) == 0:
        click.echo(
            click.style(
                'You did not specify a path to run pydoclint on.',
                fg='red',
                bold=True,
            ),
            err=echoAsError,
        )
        ctx.exit(1)

    violationsInAllFiles: dict[str, list[Violation]] = _checkPaths(
        quiet=quiet,
        exclude=exclude,
        style=style,
        paths=paths,
        argTypeHintsInSignature=arg_type_hints_in_signature,
        argTypeHintsInDocstring=arg_type_hints_in_docstring,
        checkArgOrder=check_arg_order,
        skipCheckingShortDocstrings=skip_checking_short_docstrings,
        skipCheckingRaises=skip_checking_raises,
        allowInitDocstring=allow_init_docstring,
        checkReturnTypes=check_return_types,
        checkYieldTypes=check_yield_types,
        ignoreUnderscoreArgs=ignore_underscore_args,
        ignorePrivateArgs=ignore_private_args,
        checkClassAttributes=check_class_attributes,
        shouldDocumentPrivateClassAttributes=(
            should_document_private_class_attributes
        ),
        treatPropertyMethodsAsClassAttributes=(
            treat_property_methods_as_class_attributes
        ),
        onlyAttrsWithClassVarAreTreatedAsClassAttrs=(
            only_attrs_with_classvar_are_treated_as_class_attrs
        ),
        requireReturnSectionWhenReturningNothing=(
            require_return_section_when_returning_nothing
        ),
        requireYieldSectionWhenYieldingNothing=(
            require_yield_section_when_yielding_nothing
        ),
        shouldDocumentStarArguments=should_document_star_arguments,
        shouldDeclareAssertErrorIfAssertStatementExists=(
            should_declare_assert_error_if_assert_statement_exists
        ),
        checkStyleMismatch=check_style_mismatch,
        checkArgDefaults=check_arg_defaults,
    )

    if generate_baseline:
        if baseline is None:
            click.echo(
                click.style(
                    'The baseline file was not specified. Use the'
                    ' --baseline option or specify it in your config file',
                    fg='red',
                    bold=True,
                ),
                err=echoAsError,
            )
            ctx.exit(1)

        generateBaseline(violationsInAllFiles, baselinePath)
        click.echo(
            click.style(
                'The baseline file was successfully generated',
                fg='green',
                bold=True,
            ),
            err=echoAsError,
        )
        ctx.exit(0)

    if baseline is not None:
        parsedBaseline: dict[str, list[str]] = parseBaseline(baselinePath)
        (
            baselineRegenerationNeeded,
            unfixedBaselineViolationsInAllFiles,
            violationsInAllFiles,
        ) = reEvaluateBaseline(parsedBaseline, violationsInAllFiles)
        if baselineRegenerationNeeded:
            if auto_regenerate_baseline:
                generateBaseline(
                    violationsAllFiles=unfixedBaselineViolationsInAllFiles,
                    path=baselinePath,
                )
                click.echo(
                    click.style(
                        'Some old violations were fixed, and'
                        ' the baseline file was successfully re-generated',
                        fg='green',
                        bold=True,
                    ),
                    err=echoAsError,
                )
            else:
                click.echo(
                    click.style(
                        'Some old violations were fixed. Please regenerate'
                        ' your baseline file after fixing new problems.\n'
                        'Use `--generate-baseline=True`. Or you can use'
                        ' `--auto-regenerate-baseline=True` to do this'
                        ' automatically in the future.',
                        fg='red',
                        bold=True,
                    ),
                    err=echoAsError,
                )

    # Print violation messages nicely to the terminal
    violationCounter: int = 0
    if len(violationsInAllFiles) > 0:
        counter = 0
        for filename, violationsInThisFile in violationsInAllFiles.items():
            counter += 1
            if len(violationsInThisFile) > 0:
                if counter > 1:
                    click.echo('', err=echoAsError)

                if not show_filenames_in_every_violation_message:
                    click.echo(
                        click.style(filename, fg='yellow', bold=True),
                        err=echoAsError,
                    )

                for violation in violationsInThisFile:
                    violationCounter += 1
                    if not show_filenames_in_every_violation_message:
                        fourSpaces = '    '
                        click.echo(fourSpaces, nl=False, err=echoAsError)
                    else:
                        click.echo(
                            click.style(filename, fg='yellow', bold=True),
                            nl=False,
                            err=echoAsError,
                        )
                        click.echo(':', nl=False, err=echoAsError)

                    click.echo(
                        f'{violation.line}: ', nl=False, err=echoAsError
                    )
                    click.echo(
                        click.style(
                            f'{violation.fullErrorCode}',
                            fg='red',
                            bold=True,
                        ),
                        nl=False,
                        err=echoAsError,
                    )
                    click.echo(f': {violation.msg}', err=echoAsError)

    if violationCounter > 0:
        ctx.exit(1)
    else:
        if not quiet:
            click.echo(
                click.style('🎉 No violations 🎉', fg='green', bold=True),
                err=echoAsError,
            )

        ctx.exit(0)


def _checkPaths(
        paths: tuple[str, ...],
        style: str = 'numpy',
        argTypeHintsInSignature: bool = True,
        argTypeHintsInDocstring: bool = True,
        checkArgOrder: bool = True,
        skipCheckingShortDocstrings: bool = True,
        skipCheckingRaises: bool = False,
        allowInitDocstring: bool = False,
        checkReturnTypes: bool = True,
        checkYieldTypes: bool = True,
        ignoreUnderscoreArgs: bool = True,
        ignorePrivateArgs: bool = False,
        checkClassAttributes: bool = True,
        shouldDocumentPrivateClassAttributes: bool = False,
        treatPropertyMethodsAsClassAttributes: bool = False,
        onlyAttrsWithClassVarAreTreatedAsClassAttrs: bool = False,
        requireReturnSectionWhenReturningNothing: bool = False,
        requireYieldSectionWhenYieldingNothing: bool = False,
        shouldDocumentStarArguments: bool = True,
        shouldDeclareAssertErrorIfAssertStatementExists: bool = False,
        checkStyleMismatch: bool = False,
        checkArgDefaults: bool = False,
        quiet: bool = False,
        exclude: str = '',
) -> dict[str, list[Violation]]:
    filenames: list[Path] = []

    if not quiet:
        skipMsg = f'Skipping files that match this pattern: {exclude}'
        click.echo(
            click.style(skipMsg, fg='yellow', bold=True), err=echoAsError
        )

    excludePattern = re.compile(exclude)

    for path_ in paths:
        path = Path(path_)
        if path.is_file():
            filenames.append(path)
        elif path.is_dir():
            filenames.extend(sorted(path.rglob('*.py')))

    allViolations: dict[str, list[Violation]] = {}

    for filename in filenames:
        if excludePattern.search(filename.as_posix()):
            continue

        if not quiet:
            click.echo(
                click.style(filename, fg='cyan', bold=True), err=echoAsError
            )

        violationsInThisFile: list[Violation] = _checkFile(
            filename,
            style=style,
            argTypeHintsInSignature=argTypeHintsInSignature,
            argTypeHintsInDocstring=argTypeHintsInDocstring,
            checkArgOrder=checkArgOrder,
            skipCheckingShortDocstrings=skipCheckingShortDocstrings,
            skipCheckingRaises=skipCheckingRaises,
            allowInitDocstring=allowInitDocstring,
            checkReturnTypes=checkReturnTypes,
            checkYieldTypes=checkYieldTypes,
            ignoreUnderscoreArgs=ignoreUnderscoreArgs,
            ignorePrivateArgs=ignorePrivateArgs,
            checkClassAttributes=checkClassAttributes,
            shouldDocumentPrivateClassAttributes=(
                shouldDocumentPrivateClassAttributes
            ),
            treatPropertyMethodsAsClassAttributes=(
                treatPropertyMethodsAsClassAttributes
            ),
            onlyAttrsWithClassVarAreTreatedAsClassAttrs=(
                onlyAttrsWithClassVarAreTreatedAsClassAttrs
            ),
            requireReturnSectionWhenReturningNothing=(
                requireReturnSectionWhenReturningNothing
            ),
            requireYieldSectionWhenYieldingNothing=(
                requireYieldSectionWhenYieldingNothing
            ),
            shouldDocumentStarArguments=shouldDocumentStarArguments,
            shouldDeclareAssertErrorIfAssertStatementExists=(
                shouldDeclareAssertErrorIfAssertStatementExists
            ),
            checkStyleMismatch=checkStyleMismatch,
            checkArgDefaults=checkArgDefaults,
        )
        allViolations[filename.as_posix()] = violationsInThisFile

    return allViolations


def _checkFile(
        filename: Path,
        style: str = 'numpy',
        argTypeHintsInSignature: bool = True,
        argTypeHintsInDocstring: bool = True,
        checkArgOrder: bool = True,
        skipCheckingShortDocstrings: bool = True,
        skipCheckingRaises: bool = False,
        allowInitDocstring: bool = False,
        checkReturnTypes: bool = True,
        checkYieldTypes: bool = True,
        ignoreUnderscoreArgs: bool = True,
        ignorePrivateArgs: bool = False,
        checkClassAttributes: bool = True,
        shouldDocumentPrivateClassAttributes: bool = False,
        treatPropertyMethodsAsClassAttributes: bool = False,
        onlyAttrsWithClassVarAreTreatedAsClassAttrs: bool = False,
        requireReturnSectionWhenReturningNothing: bool = False,
        requireYieldSectionWhenYieldingNothing: bool = False,
        shouldDocumentStarArguments: bool = True,
        shouldDeclareAssertErrorIfAssertStatementExists: bool = False,
        checkStyleMismatch: bool = False,
        checkArgDefaults: bool = False,
) -> list[Violation]:
    if not filename.is_file():  # sometimes folder names can end with `.py`
        return []

    with open(filename, encoding='utf-8', errors='replace') as fp:
        # Note: errors='replace' would replace unrecognized characters with
        #       question marks. This may not be a perfect solution, but for
        #       not this may be good enough.
        src: str = ''.join(fp.readlines())

    tree: ast.Module
    try:
        tree = ast.parse(src)
    except SyntaxError as e:
        if str(e).startswith('invalid non-printable character'):
            src_ = replaceInvisibleChars(src)
            try:
                # In case there's another syntax error after fixing
                # this invalid non-printable character error
                tree = ast.parse(src_)
            except SyntaxError as e2:
                return [Violation(code=2, line=0, msgPostfix=str(e2))]
        else:  # other syntax errors
            return [Violation(code=2, line=0, msgPostfix=str(e))]
    except Exception as e3:  # other non-SyntaxError exceptions
        raise e3

    visitor = Visitor(
        style=style,
        argTypeHintsInSignature=argTypeHintsInSignature,
        argTypeHintsInDocstring=argTypeHintsInDocstring,
        checkArgOrder=checkArgOrder,
        skipCheckingShortDocstrings=skipCheckingShortDocstrings,
        skipCheckingRaises=skipCheckingRaises,
        allowInitDocstring=allowInitDocstring,
        checkReturnTypes=checkReturnTypes,
        checkYieldTypes=checkYieldTypes,
        ignoreUnderscoreArgs=ignoreUnderscoreArgs,
        ignorePrivateArgs=ignorePrivateArgs,
        checkClassAttributes=checkClassAttributes,
        shouldDocumentPrivateClassAttributes=(
            shouldDocumentPrivateClassAttributes
        ),
        treatPropertyMethodsAsClassAttributes=(
            treatPropertyMethodsAsClassAttributes
        ),
        onlyAttrsWithClassVarAreTreatedAsClassAttrs=(
            onlyAttrsWithClassVarAreTreatedAsClassAttrs
        ),
        requireReturnSectionWhenReturningNothing=(
            requireReturnSectionWhenReturningNothing
        ),
        requireYieldSectionWhenYieldingNothing=(
            requireYieldSectionWhenYieldingNothing
        ),
        shouldDocumentStarArguments=shouldDocumentStarArguments,
        shouldDeclareAssertErrorIfAssertStatementExists=(
            shouldDeclareAssertErrorIfAssertStatementExists
        ),
        checkStyleMismatch=checkStyleMismatch,
        checkArgDefaults=checkArgDefaults,
    )
    visitor.visit(tree)
    return visitor.violations


if __name__ == '__main__':
    main()
