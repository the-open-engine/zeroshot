import ast

from pydoclint.utils.astTypes import FuncOrAsyncFuncDef


def checkIsAbstractMethod(node: FuncOrAsyncFuncDef) -> bool:
    """Check whether `node` is an abstract method"""
    return checkMethodContainsSpecifiedDecorator(node, 'abstractmethod')


def checkIsPropertyMethod(node: FuncOrAsyncFuncDef) -> bool:
    """Check whether `node` is a method with @property decorator"""
    return checkMethodContainsSpecifiedDecorator(node, 'property')


def checkMethodContainsSpecifiedDecorator(
        node: FuncOrAsyncFuncDef,
        decorator: str,
) -> bool:
    """Check whether a method is decorated by the specified decorator"""
    return (
        isinstance(node.decorator_list, list)
        and len(node.decorator_list) > 0
        and any(
            (  # noqa: PAR001
                isinstance(_, ast.Name)
                and hasattr(node.decorator_list[-1], 'id')
                and _.id == decorator
            )
            for _ in node.decorator_list
        )
    )
