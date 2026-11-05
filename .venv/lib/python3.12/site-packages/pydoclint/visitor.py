from __future__ import annotations

import ast

from docstring_parser import ParseError

from pydoclint.utils.arg import Arg, ArgList
from pydoclint.utils.astTypes import FuncOrAsyncFuncDef
from pydoclint.utils.doc import Doc
from pydoclint.utils.edge_case_error import EdgeCaseError
from pydoclint.utils.generic import (
    buildFuncArgToDefaultMapping,
    collectFuncArgs,
    detectMethodType,
    doList1ItemsStartWithList2Items,
    generateClassMsgPrefix,
    generateFuncMsgPrefix,
    getDocstring,
)
from pydoclint.utils.method_type import MethodType
from pydoclint.utils.parse_docstring import (
    parseDocstring,
    parseDocstringInGivenStyle,
)
from pydoclint.utils.return_anno import ReturnAnnotation
from pydoclint.utils.return_arg import ReturnArg
from pydoclint.utils.return_yield_raise import (
    getRaisedExceptions,
    hasAssertStatements,
    hasBareReturnStatements,
    hasGeneratorAsReturnAnnotation,
    hasIteratorOrIterableAsReturnAnnotation,
    hasRaiseStatements,
    hasReturnAnnotation,
    hasReturnStatements,
    hasYieldStatements,
    isReturnAnnotationNone,
    isReturnAnnotationNoReturn,
)
from pydoclint.utils.special_methods import (
    checkIsAbstractMethod,
    checkIsPropertyMethod,
)
from pydoclint.utils.unparser_custom import unparseName
from pydoclint.utils.violation import Violation
from pydoclint.utils.visitor_helper import (
    addMismatchedRaisesExceptionViolation,
    checkClassAttributesAgainstClassDocstring,
    checkDocArgsLengthAgainstActualArgs,
    checkNameOrderAndTypeHintsOfDocArgsAgainstActualArgs,
    checkReturnTypesForViolations,
    checkYieldTypesForViolations,
    extractReturnTypeFromGenerator,
    extractYieldTypeFromGeneratorOrIteratorAnnotation,
)
from pydoclint.utils.yield_arg import YieldArg


class Visitor(ast.NodeVisitor):
    """A class to recursively visit all the nodes in a parsed module"""

    def __init__(
            self,
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
    ) -> None:
        self.style: str = style
        self.argTypeHintsInSignature: bool = argTypeHintsInSignature
        self.argTypeHintsInDocstring: bool = argTypeHintsInDocstring
        self.checkArgOrder: bool = checkArgOrder
        self.skipCheckingShortDocstrings: bool = skipCheckingShortDocstrings
        self.skipCheckingRaises: bool = skipCheckingRaises
        self.allowInitDocstring: bool = allowInitDocstring
        self.checkReturnTypes: bool = checkReturnTypes
        self.checkYieldTypes: bool = checkYieldTypes
        self.ignoreUnderscoreArgs: bool = ignoreUnderscoreArgs
        self.ignorePrivateArgs: bool = ignorePrivateArgs
        self.checkClassAttributes: bool = checkClassAttributes
        self.shouldDocumentPrivateClassAttributes: bool = (
            shouldDocumentPrivateClassAttributes
        )
        self.treatPropertyMethodsAsClassAttributes: bool = (
            treatPropertyMethodsAsClassAttributes
        )
        self.onlyAttrsWithClassVarAreTreatedAsClassAttrs: bool = (
            onlyAttrsWithClassVarAreTreatedAsClassAttrs
        )
        self.requireReturnSectionWhenReturningNothing: bool = (
            requireReturnSectionWhenReturningNothing
        )
        self.requireYieldSectionWhenYieldingNothing: bool = (
            requireYieldSectionWhenYieldingNothing
        )
        self.shouldDocumentStarArguments: bool = shouldDocumentStarArguments
        self.shouldDeclareAssertErrorIfAssertStatementExists: bool = (
            shouldDeclareAssertErrorIfAssertStatementExists
        )
        self.checkStyleMismatch: bool = checkStyleMismatch
        self.checkArgDefaults: bool = checkArgDefaults

        # Validate incompatible option combination
        if self.style == 'sphinx' and self.checkArgDefaults:
            raise ValueError(
                'The option --check-arg-defaults is not compatible'
                ' with --style=sphinx.'
                ' This feature only applies to numpy and Google'
                ' style docstrings.'
            )

        self.parent: ast.AST = ast.Pass()  # keep track of parent node
        self.violations: list[Violation] = []

    def visit_ClassDef(self, node: ast.ClassDef) -> None:  # noqa: D102
        currentParent = self.parent  # keep aside
        self.parent = node

        if self.checkClassAttributes:
            checkClassAttributesAgainstClassDocstring(
                node=node,
                style=self.style,
                violations=self.violations,
                lineNum=node.lineno,
                msgPrefix=generateClassMsgPrefix(node=node, appendColon=True),
                shouldCheckArgOrder=self.checkArgOrder,
                argTypeHintsInSignature=self.argTypeHintsInSignature,
                argTypeHintsInDocstring=self.argTypeHintsInDocstring,
                skipCheckingShortDocstrings=self.skipCheckingShortDocstrings,
                shouldDocumentPrivateClassAttributes=(
                    self.shouldDocumentPrivateClassAttributes
                ),
                treatPropertyMethodsAsClassAttributes=(
                    self.treatPropertyMethodsAsClassAttributes
                ),
                onlyAttrsWithClassVarAreTreatedAsClassAttrs=(
                    self.onlyAttrsWithClassVarAreTreatedAsClassAttrs
                ),
                checkArgDefaults=self.checkArgDefaults,
            )

        self.generic_visit(node)

        self.parent = currentParent  # restore

    def visit_FunctionDef(self, node: FuncOrAsyncFuncDef) -> None:  # noqa: D102, C901
        parent_: ast.ClassDef | FuncOrAsyncFuncDef = self.parent  # type:ignore[assignment]
        self.parent = node

        isClassConstructor: bool = node.name == '__init__' and isinstance(
            parent_, ast.ClassDef
        )

        docstring: str = getDocstring(node)

        self.isAbstractMethod = checkIsAbstractMethod(node)

        if isClassConstructor:
            assert isinstance(parent_, ast.ClassDef)  # to help mypy know type
            docstring = self._checkClassDocstringAndConstructorDocstrings(
                node=node,
                parent_=parent_,
                initDocstring=docstring,
            )

        argViolations: list[Violation]
        returnViolations: list[Violation]
        yieldViolations: list[Violation]
        raiseViolations: list[Violation]

        if docstring.strip() == '':
            # We don't check functions without docstrings.
            # We defer to
            # flake8-docstrings (https://github.com/PyCQA/flake8-docstrings)
            # or pydocstyle (https://www.pydocstyle.org/en/stable/)
            # to determine whether a function needs a docstring.
            argViolations = []
            returnViolations = []
            yieldViolations = []
            raiseViolations = []
        else:
            doc: Doc
            potentialParsingError: ParseError | None
            styleMismatch: bool

            if self.checkStyleMismatch:
                doc, potentialParsingError, styleMismatch = parseDocstring(
                    docstring,
                    userSpecifiedStyle=self.style,
                )
            else:
                doc, potentialParsingError = parseDocstringInGivenStyle(
                    docstring,
                    style=self.style,
                )
                styleMismatch = False  # always silence DOC003

            if potentialParsingError is not None:
                msgPostfix: str = (
                    str(potentialParsingError).replace('\n', ' ')
                    + ' (Note: DOC001 could trigger other unrelated'
                    + ' violations under this function/method too. Please'
                    + ' fix the docstring formatting first.)'
                )
                self.violations.append(
                    Violation(
                        code=1,
                        line=node.lineno,
                        msgPrefix=f'Function/method `{node.name}`:',
                        msgPostfix=msgPostfix,
                    )
                )

            if styleMismatch:
                self.violations.append(
                    Violation(
                        code=3,
                        line=node.lineno,
                        msgPrefix=f'Function/method `{node.name}`:',
                        msgPostfix=(
                            f'You specified "{self.style}" style, but the'
                            f' docstring is likely not written in this style.'
                        ),
                    )
                )

            isShort: bool = doc.isShortDocstring
            if self.skipCheckingShortDocstrings and isShort:
                argViolations = []
                returnViolations = []
                yieldViolations = []
                raiseViolations = []
            else:
                argViolations = self.checkArguments(node, parent_, doc)
                if docstring == '':
                    returnViolations = []
                    yieldViolations = []
                    raiseViolations = []
                else:
                    if hasYieldStatements(node) and hasReturnStatements(node):
                        returnViolations = self.checkReturnAndYield(
                            node, parent_, doc
                        )
                        # It doesn't matter what violations fall into which
                        # list, so we put everything in `returnViolations`
                        # and then keep `yieldViolations` empty.
                        yieldViolations = []
                    else:
                        returnViolations = self.checkReturns(
                            node, parent_, doc
                        )
                        yieldViolations = self.checkYields(node, parent_, doc)

                    if not self.skipCheckingRaises:
                        raiseViolations = self.checkRaises(node, parent_, doc)
                    else:
                        raiseViolations = []

            if isClassConstructor:
                # Re-check return violations because the rules are
                # different for class constructors.
                returnViolations = (
                    self.checkReturnsAndYieldsInClassConstructor(
                        parent=parent_,  # type: ignore[arg-type]
                        doc=doc,
                    )
                )

        self.violations.extend(argViolations)
        self.violations.extend(returnViolations)
        self.violations.extend(yieldViolations)
        self.violations.extend(raiseViolations)

        self.generic_visit(node)

        self.parent = parent_  # restore

    def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:  # noqa: D102
        # Treat async functions similarly to regular ones
        self.visit_FunctionDef(node)

    def visit_Raise(self, node: ast.Raise) -> None:  # noqa: D102
        self.generic_visit(node)

    def _checkClassDocstringAndConstructorDocstrings(  # noqa: C901
            self,
            node: FuncOrAsyncFuncDef,
            parent_: ast.ClassDef,
            initDocstring: str,
    ) -> str:
        """
        Check class docstring and __init__() docstring.

        If only class docstring exists, or if __init__() is not allowed to have
        its own docstring, return the class docstring for further checking.

        Otherwise, return the __init__() docstring for further checking.
        """
        if not isinstance(parent_, ast.ClassDef):
            msg = (
                'This should not have happened; please contact the authors'
                ' and share the full call stack.'
            )
            raise EdgeCaseError(msg)

        className: str = parent_.name
        classLineNum: int = parent_.lineno

        classDocstring: str = getDocstring(parent_)

        if len(initDocstring) == 0:  # __init__() doesn't have own docstring
            # Check class docstring instead, because that's what we care
            # about when checking the class constructor.
            return classDocstring

        # Below: __init__() has its own docstring
        if not self.allowInitDocstring:
            self.violations.append(
                Violation(
                    code=301,
                    line=node.lineno,
                    msgPrefix=f'Class `{className}`:',
                )
            )
            return classDocstring

        # Below: __init__() is allowed to have a separate docstring
        try:
            classDoc = Doc(docstring=classDocstring, style=self.style)
        except Exception as excp:
            classDoc = Doc(docstring='', style=self.style)
            self.violations.append(
                Violation(
                    code=1,
                    line=parent_.lineno,
                    msgPrefix=f'Class `{className}`:',
                    msgPostfix=str(excp).replace('\n', ' '),
                )
            )

        try:
            initDoc = Doc(docstring=initDocstring, style=self.style)
        except Exception as excp:
            initDoc = Doc(docstring='', style=self.style)
            self.violations.append(
                Violation(
                    code=1,
                    line=node.lineno,
                    msgPrefix=f'Method `{node.name}`',
                    msgPostfix=str(excp).replace('\n', ' '),
                )
            )

        if classDoc.hasReturnsSection:
            self.violations.append(
                Violation(
                    code=302,
                    line=classLineNum,
                    msgPrefix=f'Class `{className}`:',
                )
            )

        if initDoc.hasReturnsSection:
            self.violations.append(
                Violation(
                    code=303,
                    line=node.lineno,
                    msgPrefix=f'Class `{className}`:',
                )
            )

        if classDoc.argList.nonEmpty:
            self.violations.append(
                Violation(
                    code=304,
                    line=classLineNum,
                    msgPrefix=f'Class `{className}`:',
                )
            )

        if classDoc.hasYieldsSection:
            self.violations.append(
                Violation(
                    code=306,
                    line=classLineNum,
                    msgPrefix=f'Class `{className}`:',
                )
            )

        if initDoc.hasYieldsSection:
            self.violations.append(
                Violation(
                    code=307,
                    line=classLineNum,
                    msgPrefix=f'Class `{className}`:',
                )
            )

        if classDoc.hasRaisesSection:
            self.violations.append(
                Violation(
                    code=305,
                    line=classLineNum,
                    msgPrefix=f'Class `{className}`:',
                )
            )

        return initDocstring

    def checkArguments(  # noqa: C901
            self,
            node: FuncOrAsyncFuncDef,
            parent_: ast.AST,
            doc: Doc,
    ) -> list[Violation]:
        """
        Check input arguments of the function.

        Parameters
        ----------
        node : FuncOrAsyncFuncDef
            The current function node.  It can be a regular function
            or an async function.
        parent_ : ast.AST
            The parent of the current node, which can be another function,
            a class, etc.
        doc : Doc
            The parsed docstring structure.

        Returns
        -------
        list[Violation]
            A list of argument violations
        """
        astArgList: list[ast.arg] = collectFuncArgs(node)

        if self.checkArgDefaults:
            argToDefaultMapping: dict[ast.arg, ast.expr] = (
                buildFuncArgToDefaultMapping(node)
            )

        isMethod: bool = isinstance(parent_, ast.ClassDef)
        msgPrefix: str = generateFuncMsgPrefix(node, parent_, appendColon=True)

        if isMethod:
            mType: MethodType = detectMethodType(node)
            if mType in {MethodType.INSTANCE_METHOD, MethodType.CLASS_METHOD}:
                astArgList = astArgList[1:]  # no need to document self/cls

        lineNum: int = node.lineno
        v101 = Violation(code=101, line=lineNum, msgPrefix=msgPrefix)
        v102 = Violation(code=102, line=lineNum, msgPrefix=msgPrefix)
        v104 = Violation(code=104, line=lineNum, msgPrefix=msgPrefix)

        if self.checkArgDefaults:
            v105 = Violation(
                code=105,
                line=lineNum,
                msgPrefix=msgPrefix,
                msgPostfix=(
                    '. (Note: docstring arg defaults should'
                    ' look like: `, default=XXX`)'
                ),
            )
        else:
            v105 = Violation(code=105, line=lineNum, msgPrefix=msgPrefix)

        v106 = Violation(code=106, line=lineNum, msgPrefix=msgPrefix)
        v107 = Violation(code=107, line=lineNum, msgPrefix=msgPrefix)
        v108 = Violation(code=108, line=lineNum, msgPrefix=msgPrefix)
        v109 = Violation(code=109, line=lineNum, msgPrefix=msgPrefix)
        v110 = Violation(code=110, line=lineNum, msgPrefix=msgPrefix)
        v111 = Violation(code=111, line=lineNum, msgPrefix=msgPrefix)

        docArgs: ArgList = doc.argList

        if self.checkArgDefaults:
            funcArgs = ArgList(
                [
                    Arg.fromAstArgWithMapping(_, argToDefaultMapping)
                    for _ in astArgList
                ]
            )
        else:
            funcArgs = ArgList([Arg.fromAstArg(_) for _ in astArgList])

        if self.ignoreUnderscoreArgs:
            # Ignore underscore arguments (such as _, __, ___, ...).
            # This is because these arguments are only placeholders and do not
            # need to be explained in the docstring.  (This is often used in
            # functions that must accept a certain number of input arguments.)
            funcArgs = ArgList(
                [_ for _ in funcArgs.infoList if set(_.name) != {'_'}]
            )

        if self.ignorePrivateArgs:
            # "Private arguments" are those whose names have leading
            # underscores, but whose names are not purely _, __, ___, etc.
            funcArgs = ArgList(
                [
                    _
                    for _ in funcArgs.infoList
                    if not _.name.startswith('_') or set(_.name) == {'_'}
                ]
            )

        if not self.shouldDocumentStarArguments:
            # This is "should not" rather than "need not", which means that
            # if this config option is set to False, there CANNOT be
            # documentation of star arguments in the docstring
            funcArgs = ArgList(
                [_ for _ in funcArgs.infoList if not _.name.startswith('*')]
            )

        if docArgs.length == 0 and funcArgs.length == 0:
            return []

        violations: list[Violation] = []

        checkDocArgsLengthAgainstActualArgs(
            docArgs=docArgs,
            actualArgs=funcArgs,
            violations=violations,
            violationForDocArgsLengthShorter=v101,
            violationForDocArgsLengthLonger=v102,
        )

        if self.argTypeHintsInSignature and funcArgs.noTypeHints():
            violations.append(v106)

        if (
            self.argTypeHintsInSignature
            and not funcArgs.hasTypeHintInAllArgs()
        ):
            violations.append(v107)

        if not self.argTypeHintsInSignature and funcArgs.hasTypeHintInAnyArg():
            violations.append(v108)

        if self.argTypeHintsInDocstring and (
            # A non-empty arg list is the pre-requisite for reporting DOC109.
            # Otherwise, the error message of DOC109 would not make sense.
            # ("The option `--arg-type-hints-in-docstring` is `True` but
            # there are no type hints in the docstring arg list")
            len(docArgs) > 0 and docArgs.noTypeHints()
        ):
            violations.append(v109)

        if self.argTypeHintsInDocstring and not docArgs.hasTypeHintInAllArgs():
            violations.append(v110)

        if not self.argTypeHintsInDocstring and docArgs.hasTypeHintInAnyArg():
            violations.append(v111)

        checkNameOrderAndTypeHintsOfDocArgsAgainstActualArgs(
            docArgs=docArgs,
            actualArgs=funcArgs,
            violations=violations,
            actualArgsAreClassAttributes=False,  # they are function args
            violationForOrderMismatch=v104,
            violationForTypeHintMismatch=v105,
            shouldCheckArgOrder=self.checkArgOrder,
            argTypeHintsInSignature=self.argTypeHintsInSignature,
            argTypeHintsInDocstring=self.argTypeHintsInDocstring,
            lineNum=lineNum,
            msgPrefix=msgPrefix,
        )

        return violations

    def checkReturns(  # noqa: C901
            self,
            node: FuncOrAsyncFuncDef,
            parent: ast.AST,
            doc: Doc,
    ) -> list[Violation]:
        """Check return statement & return type annotation of this function"""
        lineNum: int = node.lineno
        msgPrefix = generateFuncMsgPrefix(node, parent, appendColon=False)

        v201 = Violation(code=201, line=lineNum, msgPrefix=msgPrefix)
        v202 = Violation(code=202, line=lineNum, msgPrefix=msgPrefix)
        v203 = Violation(code=203, line=lineNum, msgPrefix=msgPrefix)

        hasReturnStmt: bool = hasReturnStatements(node)
        hasYieldStmt: bool = hasYieldStatements(node)
        hasReturnAnno: bool = hasReturnAnnotation(node)
        hasGenAsRetAnno: bool = hasGeneratorAsReturnAnnotation(node)
        onlyHasYieldStmt: bool = hasYieldStmt and not hasReturnStmt
        hasIterAsRetAnno: bool = hasIteratorOrIterableAsReturnAnnotation(node)
        isPropertyMethod: bool = checkIsPropertyMethod(node)

        docstringHasReturnSection: bool = doc.hasReturnsSection

        violations: list[Violation] = []
        if not docstringHasReturnSection and not isPropertyMethod:
            if (
                # fmt: off
                not (onlyHasYieldStmt and hasIterAsRetAnno)
                and (hasReturnStmt or (hasReturnAnno and not hasGenAsRetAnno))
                # fmt: on
            ):
                # If "Generator[...]" is put in the return type annotation,
                # we don't need a "Returns" section in the docstring. Instead,
                # we need a "Yields" section.
                if self.requireReturnSectionWhenReturningNothing:
                    violations.append(v201)
                elif (
                    # fmt: off
                    not isReturnAnnotationNone(node)
                    and not isReturnAnnotationNoReturn(node)
                    # fmt: on
                ):
                    violations.append(v201)

        if docstringHasReturnSection and not (hasReturnStmt or hasReturnAnno):
            violations.append(v202)

        if self.checkReturnTypes:
            if hasReturnAnno:
                returnAnno = ReturnAnnotation(unparseName(node.returns))
            else:
                returnAnno = ReturnAnnotation(annotation=None)

            if docstringHasReturnSection:
                returnSec: list[ReturnArg] = doc.returnSection
            else:
                returnSec = []

            if (
                returnSec == []  # no return section in docstring
                # `-> None` or `-> NoReturn` in signature
                and returnAnno.annotation in {'None', 'NoReturn'}
                and not self.requireReturnSectionWhenReturningNothing
            ):
                return violations  # no need to check return type hints at all

            if returnSec == [] and (hasGenAsRetAnno or hasIterAsRetAnno):
                # This is because if the return annotation is `Generator[...]`,
                # `Iterator[...]`, or `Iterable[...]`,
                # we don't need a "Returns" section. (Instead, we need a
                # "Yields" section in the docstring.) Therefore, we don't need
                # to check for DOC203 violations.
                return violations

            if returnSec == [] and isPropertyMethod:
                # No need to check return type for methods with "@property"
                # decorator. This is because it's OK for @property methods
                # to have no return section in the docstring.
                return violations

            checkReturnTypesForViolations(
                style=self.style,
                returnAnnotation=returnAnno,
                violationList=violations,
                returnSection=returnSec,
                violation=v203,
            )

        return violations

    @classmethod
    def checkReturnsAndYieldsInClassConstructor(
            cls,
            parent: ast.ClassDef,
            doc: Doc,
    ) -> list[Violation]:
        """Check the presence of a Returns/Yields section in class docstring"""
        violations: list[Violation] = []
        if doc.hasReturnsSection:
            violations.append(
                Violation(
                    code=302,
                    line=parent.lineno,
                    msgPrefix=f'Class `{parent.name}`:',
                )
            )

        if doc.hasYieldsSection:
            violations.append(
                Violation(
                    code=306,
                    line=parent.lineno,
                    msgPrefix=f'Class `{parent.name}`:',
                )
            )

        return violations

    def checkYields(  # noqa: C901
            self,
            node: FuncOrAsyncFuncDef,
            parent: ast.AST,
            doc: Doc,
    ) -> list[Violation]:
        """Check violations on 'yield' statements or 'Generator' annotation"""
        violations: list[Violation] = []

        lineNum: int = node.lineno
        msgPrefix = generateFuncMsgPrefix(node, parent, appendColon=False)

        v402 = Violation(code=402, line=lineNum, msgPrefix=msgPrefix)
        v403 = Violation(code=403, line=lineNum, msgPrefix=msgPrefix)
        v404 = Violation(code=404, line=lineNum, msgPrefix=msgPrefix)

        docstringHasYieldsSection: bool = doc.hasYieldsSection

        hasYieldStmt: bool = hasYieldStatements(node)
        hasGenAsRetAnno: bool = hasGeneratorAsReturnAnnotation(node)
        hasIterAsRetAnno: bool = hasIteratorOrIterableAsReturnAnnotation(node)
        noGenNorIterAsRetAnno = not hasGenAsRetAnno and not hasIterAsRetAnno

        if hasGenAsRetAnno or hasIterAsRetAnno:
            returnAnno = ReturnAnnotation(unparseName(node.returns))
        else:
            # We don't check other return annotations here, because they
            # are checked above, in `checkReturns()`.
            returnAnno = ReturnAnnotation(None)

        if not docstringHasYieldsSection:
            extract = extractYieldTypeFromGeneratorOrIteratorAnnotation
            yieldType: str | None = extract(
                returnAnnoText=returnAnno.annotation,
                hasGeneratorAsReturnAnnotation=hasGenAsRetAnno,
                hasIteratorOrIterableAsReturnAnnotation=hasIterAsRetAnno,
            )
            if hasYieldStmt:
                if (
                    yieldType == 'None'
                    and not self.requireYieldSectionWhenYieldingNothing
                ):
                    # This means that people don't need to add a "Yields"
                    # section in the docstring, if the yield type in the
                    # signature's return annotation is None.
                    pass
                else:
                    violations.append(v402)

        if docstringHasYieldsSection:
            if not hasYieldStmt or noGenNorIterAsRetAnno:
                if not self.isAbstractMethod:
                    violations.append(v403)

        if hasYieldStmt and self.checkYieldTypes:
            if docstringHasYieldsSection:
                yieldSec: list[YieldArg] = doc.yieldSection
            else:
                yieldSec = []

            checkYieldTypesForViolations(
                returnAnnotation=returnAnno,
                violationList=violations,
                yieldSection=yieldSec,
                violation=v404,
                hasGeneratorAsReturnAnnotation=hasGenAsRetAnno,
                hasIteratorOrIterableAsReturnAnnotation=hasIterAsRetAnno,
                requireYieldSectionWhenYieldingNothing=(
                    self.requireYieldSectionWhenYieldingNothing
                ),
            )

        return violations

    def checkReturnAndYield(  # noqa: C901
            self,
            node: FuncOrAsyncFuncDef,
            parent: ast.AST,
            doc: Doc,
    ) -> list[Violation]:
        """
        Check violations when a function has both `return` and `yield`
        statements in it.
        """
        """
        Here is an example of a Python function containing both `return` and
        `yield` statements:

        ```python
        from typing import Generator
        def my_function(num: int) -> Generator[int, None, str]:
            for i in range(num):
                yield i
            return 'All numbers yielded!'
        ```

        For this function, the return section of the docstring should be:

            Returns:
                str: The return value

        And the yield section of the docstring should be:

            Yields:
                int: The value being yielded
        """

        # Just a sanity check:
        assert (hasYieldStatements(node) and hasReturnStatements(node)) is True

        violations: list[Violation] = []

        lineNum: int = node.lineno
        msgPrefix = generateFuncMsgPrefix(node, parent, appendColon=False)

        v201 = Violation(code=201, line=lineNum, msgPrefix=msgPrefix)
        v203 = Violation(code=203, line=lineNum, msgPrefix=msgPrefix)
        v402 = Violation(code=402, line=lineNum, msgPrefix=msgPrefix)
        v404 = Violation(code=404, line=lineNum, msgPrefix=msgPrefix)
        v405 = Violation(code=405, line=lineNum, msgPrefix=msgPrefix)

        docstringHasReturnSection: bool = doc.hasReturnsSection
        docstringHasYieldsSection: bool = doc.hasYieldsSection

        hasGenAsRetAnno: bool = hasGeneratorAsReturnAnnotation(node)
        hasIterAsRetAnno: bool = hasIteratorOrIterableAsReturnAnnotation(node)

        hasReturnStmt: bool = hasReturnStatements(node)
        hasYieldStmt: bool = hasYieldStatements(node)
        onlyHasYieldStmt: bool = hasYieldStmt and not hasReturnStmt
        hasReturnAnno: bool = hasReturnAnnotation(node)

        if hasReturnStmt:
            hasBareReturnStmt: bool = hasBareReturnStatements(node)
        else:
            hasBareReturnStmt = False  # to save some time

        returnAnno = ReturnAnnotation(unparseName(node.returns))
        returnSec: list[ReturnArg] = doc.returnSection

        # Check the return section in the docstring
        retTypeInGenerator: str | None
        if not docstringHasReturnSection:
            if doc.isShortDocstring and self.skipCheckingShortDocstrings:
                pass
            else:
                if (
                    # fmt: off
                    not (onlyHasYieldStmt and hasIterAsRetAnno)
                    and (
                        hasReturnStmt
                        or (hasReturnAnno and not hasGenAsRetAnno)
                    )
                    # If the return statement in the function body is a bare
                    # return, we don't throw DOC201 or DOC405. See more at:
                    # https://github.com/jsh9/pydoclint/issues/126#issuecomment-2136497913
                    and not hasBareReturnStmt
                    # fmt: on
                ):
                    retTypeInGenerator = extractReturnTypeFromGenerator(
                        returnAnnoText=returnAnno.annotation,
                    )
                    # If "Generator[...]" is put in the return type annotation,
                    # we don't need a "Returns" section in the docstring.
                    # Instead, we need a "Yields" section.
                    if self.requireReturnSectionWhenReturningNothing:
                        violations.append(v201)
                    elif retTypeInGenerator not in {'None', 'NoReturn'}:
                        violations.append(v201)
        else:
            if self.checkReturnTypes:
                if hasGenAsRetAnno:
                    retTypeInGenerator = extractReturnTypeFromGenerator(
                        returnAnnoText=returnAnno.annotation,
                    )
                    checkReturnTypesForViolations(
                        style=self.style,
                        returnAnnotation=ReturnAnnotation(retTypeInGenerator),
                        violationList=violations,
                        returnSection=returnSec,
                        violation=v203,
                    )
                else:
                    violations.append(v405)
            else:
                if not hasGenAsRetAnno:
                    violations.append(v405)

        # Check the yield section in the docstring
        if not docstringHasYieldsSection:
            if not self.skipCheckingShortDocstrings:
                violations.append(v402)
        else:
            if self.checkYieldTypes:
                returnAnno = ReturnAnnotation(unparseName(node.returns))
                yieldSec: list[YieldArg] = doc.yieldSection

                if hasGenAsRetAnno or hasIterAsRetAnno:
                    extract = extractYieldTypeFromGeneratorOrIteratorAnnotation
                    yieldType: str | None = extract(
                        returnAnnoText=returnAnno.annotation,
                        hasGeneratorAsReturnAnnotation=hasGenAsRetAnno,
                        hasIteratorOrIterableAsReturnAnnotation=hasIterAsRetAnno,  # noqa: LN001
                    )
                    checkYieldTypesForViolations(
                        returnAnnotation=ReturnAnnotation(yieldType),
                        violationList=violations,
                        yieldSection=yieldSec,
                        violation=v404,
                        hasGeneratorAsReturnAnnotation=hasGenAsRetAnno,
                        hasIteratorOrIterableAsReturnAnnotation=hasIterAsRetAnno,  # noqa: LN001
                        requireYieldSectionWhenYieldingNothing=(
                            self.requireYieldSectionWhenYieldingNothing
                        ),
                    )
                else:
                    violations.append(v405)
            else:
                if (
                    not hasGenAsRetAnno or not hasIterAsRetAnno
                ) and not hasBareReturnStmt:
                    violations.append(v405)

        return violations

    def checkRaises(  # noqa: C901
            self,
            node: FuncOrAsyncFuncDef,
            parent: ast.AST,
            doc: Doc,
    ) -> list[Violation]:
        """Check violations on 'raise' statements"""
        violations: list[Violation] = []

        lineNum: int = node.lineno
        msgPrefix = generateFuncMsgPrefix(node, parent, appendColon=False)

        v501 = Violation(code=501, line=lineNum, msgPrefix=msgPrefix)
        v502 = Violation(code=502, line=lineNum, msgPrefix=msgPrefix)
        v503 = Violation(code=503, line=lineNum, msgPrefix=msgPrefix)
        v504 = Violation(code=504, line=lineNum, msgPrefix=msgPrefix)

        docstringHasRaisesSection: bool = doc.hasRaisesSection
        hasRaiseStmt: bool = hasRaiseStatements(node)
        hasAssertStmt: bool = hasAssertStatements(node)

        if hasRaiseStmt and not docstringHasRaisesSection:
            violations.append(v501)

        if self.shouldDeclareAssertErrorIfAssertStatementExists:
            if (
                not hasAssertStmt
                and not hasRaiseStmt
                and docstringHasRaisesSection
                and not self.isAbstractMethod
            ):
                violations.append(v502)
        else:
            if (
                not hasRaiseStmt
                and docstringHasRaisesSection
                and not self.isAbstractMethod
            ):
                violations.append(v502)

        if self.shouldDeclareAssertErrorIfAssertStatementExists:
            if hasAssertStmt and not docstringHasRaisesSection:
                violations.append(v504)

        # check that the raise statements match those in body.
        if hasRaiseStmt:
            docRaises: list[str] = []

            for raises in doc.parsed.raises:
                if raises.type_name:
                    docRaises.append(raises.type_name)
                elif doc.style == 'sphinx' and raises.description:
                    # :raises: Exception: -> 'Exception'
                    splitDesc = raises.description.split(':')
                    if len(splitDesc) > 1 and ' ' not in splitDesc[0].strip():
                        exc = splitDesc[0].strip()
                        docRaises.append(exc)

            docRaises.sort()
            actualRaises: list[str] = getRaisedExceptions(
                node,
                self.shouldDeclareAssertErrorIfAssertStatementExists,
            )

            if not doList1ItemsStartWithList2Items(actualRaises, docRaises):
                # We only do partial string comparison because there are
                # cases like `raise a.b.c.MyException.e.f(1, 2)`, where the
                # expected docstring exception is `a.b.c.MyException`, but
                # there isn't an effective way to cleanly remove `e.f` at the
                # end solely based on AST manipulation.
                addMismatchedRaisesExceptionViolation(
                    docRaises=docRaises,
                    actualRaises=actualRaises,
                    violations=violations,
                    violationForRaisesMismatch=v503,
                    lineNum=lineNum,
                    msgPrefix=msgPrefix,
                )

        return violations
