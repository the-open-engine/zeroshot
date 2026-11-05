# mypy: disable-error-code=attr-defined
from __future__ import annotations

import ast
import importlib.metadata as importlib_metadata
from typing import Any, Generator

from pydoclint.visitor import Visitor


class Plugin:
    """Flake8 plugin entry point"""

    name = 'pydoclint'
    version = importlib_metadata.version(name)

    def __init__(self, tree: ast.AST) -> None:
        self._tree = tree

    @classmethod
    def add_options(cls, parser: Any) -> None:  # noqa: D102
        parser.add_option(
            '--style',
            action='store',
            default='numpy',
            parse_from_config=True,
            help='Which style of docstring is your code base using',
        )
        parser.add_option(
            '-ths',
            '--type-hints-in-signature',
            action='store',
            default='None',
            parse_from_config=True,
            help='(Deprecated) Please use --arg-type-hints-in-signature instead',
        )
        parser.add_option(
            '-aths',
            '--arg-type-hints-in-signature',
            action='store',
            default='True',
            parse_from_config=True,
            help='Whether to require argument type hints in function signatures',
        )
        parser.add_option(
            '-thd',
            '--type-hints-in-docstring',
            action='store',
            default='None',
            parse_from_config=True,
            help='(Deprecated) Please use --arg-type-hints-in-docstring instead',
        )
        parser.add_option(
            '-athd',
            '--arg-type-hints-in-docstring',
            action='store',
            default='True',
            parse_from_config=True,
            help='Whether to require type hints in the argument list in docstrings',
        )
        parser.add_option(
            '-ao',
            '--check-arg-order',
            action='store',
            default='True',
            parse_from_config=True,
            help=(
                'Whether to check argument orders in the docstring'
                ' against the argument list in the function signature'
            ),
        )
        parser.add_option(
            '-scsd',
            '--skip-checking-short-docstrings',
            action='store',
            default='True',
            parse_from_config=True,
            help='If True, skip checking if the docstring only has a short summary.',
        )
        parser.add_option(
            '-scr',
            '--skip-checking-raises',
            action='store',
            default='False',
            parse_from_config=True,
            help='If True, skip checking docstring "Raises" section against "raise" statements',
        )
        parser.add_option(
            '-aid',
            '--allow-init-docstring',
            action='store',
            default='False',
            parse_from_config=True,
            help='If True, allow both __init__() and the class def to have docstrings',
        )
        parser.add_option(
            '--require-return-section-when-returning-none',
            action='store',
            default='None',
            parse_from_config=True,
            help=(
                '(Deprecated) Please use'
                ' --require-return-section-when-returning-nothing instead.'
            ),
        )
        parser.add_option(
            '-rrs',
            '--require-return-section-when-returning-nothing',
            action='store',
            default='False',
            parse_from_config=True,
            help=(
                'If False, a return section is not needed in docstring if'
                ' the function body does not have a "return" statement and'
                ' the return type annotation is "-> None" or "-> NoReturn".'
            ),
        )
        parser.add_option(
            '-crt',
            '--check-return-types',
            action='store',
            default='True',
            parse_from_config=True,
            help=(
                'If True, check that the type(s) in the docstring return section and'
                ' the return annotation in the function signature are consistent'
            ),
        )
        parser.add_option(
            '-rys',
            '--require-yield-section-when-yielding-nothing',
            action='store',
            default='False',
            parse_from_config=True,
            help=(
                'If False, a yields section is not needed in docstring if'
                ' the function yields None.'
            ),
        )
        parser.add_option(
            '-cyt',
            '--check-yield-types',
            action='store',
            default='True',
            parse_from_config=True,
            help=(
                'If True, check that the type(s) in the docstring "yields" section and'
                ' the return annotation in the function signature are consistent'
            ),
        )
        parser.add_option(
            '-iua',
            '--ignore-underscore-args',
            action='store',
            default='True',
            parse_from_config=True,
            help=(
                'If True, underscore arguments (such as _, __, ...) in the function'
                ' signature do not need to appear in the docstring. Note: "underscore'
                ' arguments" are not the same as "arguments with leading'
                ' underscores" (such as `_a`).'
            ),
        )
        parser.add_option(
            '-ipa',
            '--ignore-private-args',
            action='store',
            default='False',
            parse_from_config=True,
            help=(
                'If True, private arguments (those with leading underscores '
                ' in their names but are not purely `_`, `__`, etc.) in the'
                ' function signature do not need to appear in the docstring.'
            ),
        )
        parser.add_option(
            '-cca',
            '--check-class-attributes',
            action='store',
            default='True',
            parse_from_config=True,
            help=(
                'If True, class attributes (the ones defined right beneath'
                ' "class MyClass:") are checked against the docstring.'
            ),
        )
        parser.add_option(
            '-sdpca',
            '--should-document-private-class-attributes',
            action='store',
            default='False',
            parse_from_config=True,
            help=(
                'If True, private class attributes (the ones starting with _)'
                ' should be documented in the docstring. If False, private'
                ' class attributes should not appear in the docstring.'
            ),
        )
        parser.add_option(
            '-tpmaca',
            '--treat-property-methods-as-class-attributes',
            action='store',
            default='False',
            parse_from_config=True,
            help=(
                'If True, treat @property methods as class properties. This means'
                ' that they need to be documented in the "Attributes" section of'
                ' the class docstring, and there cannot be any docstring under'
                ' the @property methods. This option is only effective when'
                ' --check-class-attributes is True. We recommend setting both'
                ' this option and --check-class-attributes to True.'
            ),
        )
        parser.add_option(
            '-oawcv',
            '--only-attrs-with-ClassVar-are-treated-as-class-attrs',
            action='store',
            default='False',
            parse_from_config=True,
            help=(
                'If True, only the attributes whose type annotations are wrapped'
                ' within `ClassVar` (where `ClassVar` is imported from `typing`)'
                ' are treated as class attributes, and all other attributes are'
                ' treated as instance attributes.'
            ),
        )
        parser.add_option(
            '-sdsa',
            '--should-document-star-arguments',
            action='store',
            default='True',
            parse_from_config=True,
            help=(
                'If True, "star arguments" (such as *args, **kwargs,'
                ' **props, etc.) in the function signature should be'
                ' documented in the docstring. If False, they should not'
                ' appear in the docstring.'
            ),
        )
        parser.add_option(
            '-sdae',
            '--should-declare-assert-error-if-assert-statement-exists',
            action='store',
            default='False',
            help=(
                'If True, we need to declare AssertError in the "Raises"'
                ' section of the docstring if there are any "assert"'
                ' statements in the function body. This is because an "assert"'
                ' statement could raise an AssertError.'
            ),
        )
        parser.add_option(
            '-csm',
            '--check-style-mismatch',
            action='store',
            default='False',
            parse_from_config=True,
            help=(
                'If True, check that style specified in --style matches the detected'
                ' style of the docstring. If there is a mismatch, DOC003 will be'
                ' reported. Setting this to False will silence all DOC003 violations.'
            ),
        )
        parser.add_option(
            '-cad',
            '--check-arg-defaults',
            action='store',
            default='False',
            parse_from_config=True,
            help=(
                'If True, docstring type hints should contain default values '
                'consistent with the function signature. If False, docstring '
                'type hints should not contain default values. (Only applies '
                'to numpy style for now.)'
            ),
        )

    @classmethod
    def parse_options(cls, options: Any) -> None:  # noqa: D102
        cls.type_hints_in_signature = options.type_hints_in_signature
        cls.type_hints_in_docstring = options.type_hints_in_docstring
        cls.arg_type_hints_in_signature = options.arg_type_hints_in_signature
        cls.arg_type_hints_in_docstring = options.arg_type_hints_in_docstring
        cls.check_arg_order = options.check_arg_order
        cls.skip_checking_short_docstrings = (
            options.skip_checking_short_docstrings
        )
        cls.skip_checking_raises = options.skip_checking_raises
        cls.allow_init_docstring = options.allow_init_docstring
        cls.require_return_section_when_returning_none = (
            options.require_return_section_when_returning_none
        )
        cls.require_return_section_when_returning_nothing = (
            options.require_return_section_when_returning_nothing
        )
        cls.require_yield_section_when_yielding_nothing = (
            options.require_yield_section_when_yielding_nothing
        )
        cls.check_return_types = options.check_return_types
        cls.check_yield_types = options.check_yield_types
        cls.ignore_underscore_args = options.ignore_underscore_args
        cls.ignore_private_args = options.ignore_private_args
        cls.check_class_attributes = options.check_class_attributes
        cls.should_document_private_class_attributes = (
            options.should_document_private_class_attributes
        )
        cls.treat_property_methods_as_class_attributes = (
            options.treat_property_methods_as_class_attributes
        )
        cls.only_attrs_with_ClassVar_are_treated_as_class_attrs = (
            options.only_attrs_with_ClassVar_are_treated_as_class_attrs
        )
        cls.should_document_star_arguments = (
            options.should_document_star_arguments
        )
        cls.check_style_mismatch = options.check_style_mismatch
        cls.check_arg_defaults = options.check_arg_defaults
        cls.style = options.style

    def run(self) -> Generator[tuple[int, int, str, Any], None, None]:
        """Run the linter and yield the violation information"""
        if self.type_hints_in_docstring != 'None':  # user supplies this option
            raise ValueError(
                'The option `--type-hints-in-docstring` has been renamed;'
                ' please use `--arg-type-hints-in-docstring` instead'
            )

        if self.type_hints_in_signature != 'None':  # user supplies this option
            raise ValueError(
                'The option `--type-hints-in-signature` has been renamed;'
                ' please use `--arg-type-hints-in-signature` instead'
            )

        # user supplies this option
        if self.require_return_section_when_returning_none != 'None':
            raise ValueError(
                'The option `--require-return-section-when-returning-none`'
                ' has been renamed; please use'
                '`--require-return-section-when-returning-nothing` instead'
            )

        argTypeHintsInSignature = self._bool(
            '--arg-type-hints-in-signature',
            self.arg_type_hints_in_signature,
        )
        argTypeHintsInDocstring = self._bool(
            '--arg-type-hints-in-docstring',
            self.arg_type_hints_in_docstring,
        )
        checkArgOrder = self._bool('--check-arg-order', self.check_arg_order)
        skipCheckingShortDocstrings = self._bool(
            '--skip-checking-short-docstrings',
            self.skip_checking_short_docstrings,
        )
        skipCheckingRaises = self._bool(
            '--skip-checking-raises',
            self.skip_checking_raises,
        )
        allowInitDocstring = self._bool(
            '--allow-init-docstring',
            self.allow_init_docstring,
        )
        requireReturnSectionWhenReturningNothing = self._bool(
            '--require-return-section-when-returning-nothing',
            self.require_return_section_when_returning_nothing,
        )
        requireYieldSectionWhenYieldingNothing = self._bool(
            '--require-yield-section-when-yielding-nothing',
            self.require_yield_section_when_yielding_nothing,
        )
        checkReturnTypes = self._bool(
            '--check-return-types',
            self.check_return_types,
        )
        checkYieldTypes = self._bool(
            '--check-yield-types',
            self.check_yield_types,
        )
        ignoreUnderscoreArgs = self._bool(
            '--ignore-underscore-args',
            self.ignore_underscore_args,
        )
        ignorePrivateArgs = self._bool(
            '--ignore-private-args',
            self.ignore_private_args,
        )
        checkClassAttributes = self._bool(
            '--check-class-attributes',
            self.check_class_attributes,
        )
        shouldDocumentPrivateClassAttributes = self._bool(
            '--should-document-private-class-attributes',
            self.should_document_private_class_attributes,
        )
        treatPropertyMethodsAsClassAttributes = self._bool(
            '--treat-property-methods-as-class-attributes',
            self.treat_property_methods_as_class_attributes,
        )
        onlyAttrsWithClassVarAreTreatedAsClassAttrs = self._bool(
            '--only-attrs-with-ClassVar-are-treated-as-class-attrs',
            self.only_attrs_with_ClassVar_are_treated_as_class_attrs,
        )
        shouldDocumentStarArguments = self._bool(
            '--should-document-star-arguments',
            self.should_document_star_arguments,
        )
        checkStyleMismatch = self._bool(
            '--check-style-mismatch',
            self.check_style_mismatch,
        )
        checkArgDefaults = self._bool(
            '--check-arg-defaults',
            self.check_arg_defaults,
        )

        if self.style not in {'numpy', 'google', 'sphinx'}:
            raise ValueError(
                'Invalid value for "--style": must be "numpy", "google", or "sphinx"'
            )

        v = Visitor(
            argTypeHintsInSignature=argTypeHintsInSignature,
            argTypeHintsInDocstring=argTypeHintsInDocstring,
            checkArgOrder=checkArgOrder,
            skipCheckingShortDocstrings=skipCheckingShortDocstrings,
            skipCheckingRaises=skipCheckingRaises,
            allowInitDocstring=allowInitDocstring,
            requireReturnSectionWhenReturningNothing=(
                requireReturnSectionWhenReturningNothing
            ),
            requireYieldSectionWhenYieldingNothing=(
                requireYieldSectionWhenYieldingNothing
            ),
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
            shouldDocumentStarArguments=shouldDocumentStarArguments,
            checkStyleMismatch=checkStyleMismatch,
            checkArgDefaults=checkArgDefaults,
            style=self.style,
        )
        v.visit(self._tree)
        violationInfo = [_.getInfoForFlake8() for _ in v.violations]
        for line, colOffset, msg in violationInfo:
            yield line, colOffset, msg, type(self)

    @classmethod
    def _bool(cls, optionName: str, optionValue: str) -> bool:
        if optionValue == 'True':
            return True

        if optionValue == 'False':
            return False

        raise ValueError(f'Invalid argument value: {optionName}={optionValue}')
