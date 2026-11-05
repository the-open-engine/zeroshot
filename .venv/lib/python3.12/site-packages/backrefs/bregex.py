r"""
Backrefs for the 'regex' module.

Add the ability to use the following backrefs with re:

 - `\Q` and `\Q...\E`                                           - Escape/quote chars (search)
 - `\c` and `\C...\E`                                           - Uppercase char or chars (replace)
 - `\l` and `\L...\E`                                           - Lowercase char or chars (replace)
 - `\N{Black Club Suit}`                                        - Unicode character by name (replace)
 - `\u0000` and `\U00000000`                                    - Unicode characters (replace)
 - `\R`                                                         - Generic line breaks (search)

Licensed under MIT
Copyright (c) 2015 - 2020 Isaac Muse <isaacmuse@gmail.com>
"""
from __future__ import annotations
import regex as _regex  # type: ignore[import]
import copyreg as _copyreg
from functools import lru_cache as _lru_cache
from . import util as _util
from .util import PatternError
from . import _bregex_parse
from ._bregex_parse import ReplaceTemplate
from typing import AnyStr, Callable, Any, Generic, Mapping, Iterator, cast
from ._bregex_typing import Pattern, Match

__all__ = (
    "expand", "expandf", "match", "fullmatch", "search", "sub", "subf", "subn", "subfn", "split", "splititer",
    "findall", "finditer", "purge", "escape", "D", "DEBUG", "A", "ASCII", "B", "BESTMATCH",
    "E", "ENHANCEMATCH", "F", "FULLCASE", "I", "IGNORECASE", "L", "LOCALE", "M", "MULTILINE", "R", "REVERSE",
    "S", "DOTALL", "U", "UNICODE", "X", "VERBOSE", "V0", "VERSION0", "V1", "VERSION1", "W", "WORD",
    "P", "POSIX", "DEFAULT_VERSION", "FORMAT", "compile", "compile_search", "compile_replace", "Bregex",
    "ReplaceTemplate", "PatternError"
)

# Expose some common re flags and methods to
# save having to import re and backrefs libraries
D = _regex.D
DEBUG = _regex.DEBUG
A = _regex.A
ASCII = _regex.ASCII
B = _regex.B
BESTMATCH = _regex.BESTMATCH
E = _regex.E
ENHANCEMATCH = _regex.ENHANCEMATCH
F = _regex.F
FULLCASE = _regex.FULLCASE
I = _regex.I
IGNORECASE = _regex.IGNORECASE
L = _regex.L
LOCALE = _regex.LOCALE
M = _regex.M
MULTILINE = _regex.MULTILINE
R = _regex.R
REVERSE = _regex.REVERSE
S = _regex.S
DOTALL = _regex.DOTALL
U = _regex.U
UNICODE = _regex.UNICODE
X = _regex.X
VERBOSE = _regex.VERBOSE
V0 = _regex.V0
VERSION0 = _regex.VERSION0
V1 = _regex.V1
VERSION1 = _regex.VERSION1
W = _regex.W
WORD = _regex.WORD
P = _regex.P
POSIX = _regex.POSIX
DEFAULT_VERSION = _regex.DEFAULT_VERSION
escape = _regex.escape

# Replace flags
FORMAT = 1

# Case upper or lower
_UPPER = 1
_LOWER = 2

# Maximum size of the cache.
_MAXCACHE = 500

_REGEX_TYPE = type(_regex.compile('', 0))


@_lru_cache(maxsize=_MAXCACHE)
def _cached_search_compile(
    pattern: AnyStr,
    re_verbose: bool,
    re_version: bool,
    pattern_type: type[AnyStr]
) -> AnyStr:
    """Cached search compile."""

    return _bregex_parse._SearchParser(pattern, re_verbose, re_version).parse()


@_lru_cache(maxsize=_MAXCACHE)
def _cached_replace_compile(
    pattern: Pattern[AnyStr],
    repl: AnyStr,
    flags: int,
    pattern_type: type[AnyStr]
) -> ReplaceTemplate[AnyStr]:
    """Cached replace compile."""

    return _bregex_parse._ReplaceParser(pattern, repl, bool(flags & FORMAT)).parse()


def _get_cache_size(replace: bool = False) -> int:
    """Get size of cache."""

    if not replace:
        size = _cached_search_compile.cache_info().currsize
    else:
        size = _cached_replace_compile.cache_info().currsize
    return size


def _purge_cache() -> None:
    """Purge the cache."""

    _cached_replace_compile.cache_clear()
    _cached_search_compile.cache_clear()


def _is_replace(obj: Any) -> bool:
    """Check if object is a replace object."""

    return isinstance(obj, ReplaceTemplate)


def _apply_replace_backrefs(
    m: Match[AnyStr] | None,
    repl: ReplaceTemplate[AnyStr] | AnyStr,
    flags: int = 0
) -> AnyStr:
    """Expand with either the `ReplaceTemplate` or compile on the fly, or return None."""

    if m is None:
        raise ValueError("Match is None!")

    if isinstance(repl, ReplaceTemplate):
        return repl.expand(m)
    return _bregex_parse._ReplaceParser(m.re, repl, bool(flags & FORMAT)).parse().expand(m)


def _apply_search_backrefs(
    pattern: AnyStr | Pattern[AnyStr] | Bregex[AnyStr],
    flags: int = 0
) -> AnyStr | Pattern[AnyStr]:
    """Apply the search backrefs to the search pattern."""

    if isinstance(pattern, (str, bytes)):
        re_verbose = VERBOSE & flags
        if flags & V0:
            re_version = V0
        elif flags & V1:
            re_version = V1
        else:
            re_version = 0
        if not (flags & DEBUG):
            p = _cached_search_compile(
                pattern, re_verbose, re_version, type(pattern)
            )  # type: AnyStr | Pattern[AnyStr]
        else:  # pragma: no cover
            p = _bregex_parse._SearchParser(cast(AnyStr, pattern), re_verbose, re_version).parse()
    elif isinstance(pattern, Bregex):
        if flags:
            raise ValueError("Cannot process flags argument with a compiled pattern")
        p = pattern._pattern
    elif isinstance(pattern, _REGEX_TYPE):
        if flags:
            raise ValueError("Cannot process flags argument with a compiled pattern!")
        p = pattern
    else:
        raise TypeError("Not a string or compiled pattern!")
    return p


def _assert_expandable(repl: Any, use_format: bool = False) -> None:
    """Check if replace template is expandable."""

    if isinstance(repl, ReplaceTemplate):
        if repl.use_format != use_format:
            if use_format:
                raise ValueError("Replace not compiled as a format replace")
            else:
                raise ValueError("Replace should not be compiled as a format replace!")
    elif not isinstance(repl, (str, bytes)):
        raise TypeError("Expected string, buffer, or compiled replace!")


###########################
# API
##########################
class Bregex(_util.Immutable, Generic[AnyStr]):
    """Bregex object."""

    _pattern: Pattern[AnyStr]
    auto_compile: bool
    _hash: int

    __slots__ = ("_pattern", "auto_compile", "_hash")

    def __init__(self, pattern: Pattern[AnyStr], auto_compile: bool = True) -> None:
        """Initialization."""

        super().__init__(
            _pattern=pattern,
            auto_compile=auto_compile,
            _hash=hash((type(self), type(pattern), pattern, auto_compile))
        )

    @property
    def pattern(self) -> AnyStr:
        """Return pattern."""

        return cast(AnyStr, self._pattern.pattern)

    @property
    def flags(self) -> int:
        """Return flags."""

        return cast(int, self._pattern.flags)

    @property
    def groupindex(self) -> Mapping[str, int]:
        """Return group index."""

        return cast(Mapping[str, int], self._pattern.groupindex)

    @property
    def groups(self) -> tuple[AnyStr | None, ...]:
        """Return groups."""

        return cast('tuple[AnyStr | None, ...]', self._pattern.groups)

    @property
    def scanner(self) -> Any:
        """Return scanner."""

        return self._pattern.scanner

    def __hash__(self) -> int:
        """Hash."""

        return self._hash

    def __eq__(self, other: Any) -> bool:
        """Equal."""

        return (
            isinstance(other, Bregex) and
            self._pattern == other._pattern and
            self.auto_compile == other.auto_compile
        )

    def __ne__(self, other: Any) -> bool:
        """Equal."""

        return (
            not isinstance(other, Bregex) or
            self._pattern != other._pattern or
            self.auto_compile != other.auto_compile
        )

    def __repr__(self) -> str:  # pragma: no cover
        """Representation."""

        return '{}.{}({!r}, auto_compile={!r})'.format(
            self.__module__, self.__class__.__name__, self._pattern, self.auto_compile
        )

    def _auto_compile(
        self,
        template: AnyStr | Callable[..., AnyStr],
        use_format: bool = False
    ) -> AnyStr | Callable[..., AnyStr]:
        """Compile replacements."""

        if isinstance(template, ReplaceTemplate):
            if use_format != template.use_format:
                raise ValueError("Compiled replace cannot be a format object!")
        elif isinstance(template, ReplaceTemplate) or (isinstance(template, (str, bytes)) and self.auto_compile):
            return self.compile(template, (FORMAT if use_format and not isinstance(template, ReplaceTemplate) else 0))
        elif isinstance(template, (str, bytes)) and use_format:
            # Reject an attempt to run format replace when auto-compiling
            # of template strings has been disabled and we are using a
            # template string.
            raise AttributeError('Format replaces cannot be called without compiling replace template!')
        return template

    def compile(  # noqa A001
        self,
        repl: AnyStr | Callable[..., AnyStr],
        flags: int = 0
    ) -> Callable[..., AnyStr]:
        """Compile replace."""

        return compile_replace(self._pattern, repl, flags)

    @property
    def named_lists(self) -> Mapping[str, set[str | bytes]]:
        """Returned named lists."""

        return cast('Mapping[str, set[str | bytes]]', self._pattern.named_lists)

    def search(
        self,
        string: AnyStr,
        *args: Any,
        **kwargs: Any
    ) -> Match[AnyStr] | None:
        """Apply `search`."""

        return self._pattern.search(string, *args, **kwargs)

    def match(
        self,
        string: AnyStr,
        *args: Any,
        **kwargs: Any
    ) -> Match[AnyStr] | None:
        """Apply `match`."""

        return cast('Match[AnyStr] | None', self._pattern.match(string, *args, **kwargs))

    def fullmatch(
        self,
        string: AnyStr,
        *args: Any,
        **kwargs: Any
    ) -> Match[AnyStr] | None:
        """Apply `fullmatch`."""

        return cast('Match[AnyStr] | None', self._pattern.fullmatch(string, *args, **kwargs))

    def split(
        self,
        string: AnyStr,
        *args: Any,
        **kwargs: Any
    ) -> list[AnyStr]:
        """Apply `split`."""

        return cast('list[AnyStr]', self._pattern.split(string, *args, **kwargs))

    def splititer(
        self,
        string: AnyStr,
        *args: Any,
        **kwargs: Any
    ) -> Iterator[AnyStr]:
        """Apply `splititer`."""

        return cast(Iterator[AnyStr], self._pattern.splititer(string, *args, **kwargs))

    def findall(
        self,
        string: AnyStr,
        *args: Any,
        **kwargs: Any
    ) -> list[AnyStr] | list[tuple[AnyStr, ...]]:
        """Apply `findall`."""

        return cast('list[AnyStr] | list[tuple[AnyStr, ...]]', self._pattern.findall(string, *args, **kwargs))

    def finditer(
        self,
        string: AnyStr,
        *args: Any,
        **kwargs: Any
    ) -> Iterator[Match[AnyStr]]:
        """Apply `finditer`."""

        return cast(Iterator[Match[AnyStr]], self._pattern.finditer(string, *args, **kwargs))

    def sub(
        self,
        repl: AnyStr | Callable[..., AnyStr],
        string: AnyStr,
        *args: Any,
        **kwargs: Any
    ) -> AnyStr:
        """Apply `sub`."""

        return cast(AnyStr, self._pattern.sub(self._auto_compile(repl), string, *args, **kwargs))

    def subf(
        self,
        repl: AnyStr | Callable[..., AnyStr],
        string: AnyStr,
        *args: Any,
        **kwargs: Any
    ) -> AnyStr:  # noqa A002
        """Apply `sub` with format style replace."""

        return cast(AnyStr, self._pattern.subf(self._auto_compile(repl, True), string, *args, **kwargs))

    def subn(
        self,
        repl: AnyStr | Callable[..., AnyStr],
        string: AnyStr,
        *args: Any,
        **kwargs: Any
    ) -> tuple[AnyStr, int]:
        """Apply `subn` with format style replace."""

        return cast('tuple[AnyStr, int]', self._pattern.subn(self._auto_compile(repl), string, *args, **kwargs))

    def subfn(
        self,
        repl: AnyStr | Callable[..., AnyStr],
        string: AnyStr,
        *args: Any,
        **kwargs: Any
    ) -> tuple[AnyStr, int]:  # noqa A002
        """Apply `subn` after applying backrefs."""

        return cast('tuple[AnyStr, int]', self._pattern.subfn(self._auto_compile(repl, True), string, *args, **kwargs))


def compile(  # noqa A001
    pattern: AnyStr | Pattern[AnyStr] | Bregex[AnyStr],
    flags: int = 0,
    auto_compile: bool | None = None,
    **kwargs: Any
) -> Bregex[AnyStr]:
    """Compile both the search or search and replace into one object."""

    if isinstance(pattern, Bregex):
        if auto_compile is not None:
            raise ValueError("Cannot compile Bregex with a different auto_compile!")
        elif flags != 0:
            raise ValueError("Cannot process flags argument with a compiled pattern")
        return pattern
    else:
        if auto_compile is None:
            auto_compile = True

        return Bregex(compile_search(pattern, flags, **kwargs), auto_compile)


def compile_search(
    pattern: AnyStr | Pattern[AnyStr] | Bregex[AnyStr],
    flags: int = 0,
    **kwargs: Any
) -> Pattern[AnyStr]:
    """Compile with extended search references."""

    return cast(Pattern[AnyStr], _regex.compile(_apply_search_backrefs(pattern, flags), flags, **kwargs))


def compile_replace(
    pattern: Pattern[AnyStr],
    repl: AnyStr | Callable[..., AnyStr],
    flags: int = 0
) -> Callable[..., AnyStr]:
    """Construct a method that can be used as a replace method for `sub`, `subn`, etc."""

    if pattern is not None and isinstance(pattern, _REGEX_TYPE):
        if isinstance(repl, (str, bytes)):
            if not (pattern.flags & DEBUG):
                call = _cached_replace_compile(pattern, repl, flags, type(repl))
            else:  # pragma: no cover
                call = _bregex_parse._ReplaceParser(pattern, repl, bool(flags & FORMAT)).parse()
        elif isinstance(repl, ReplaceTemplate):
            if flags:
                raise ValueError("Cannot process flags argument with a ReplaceTemplate!")
            if repl.pattern_hash != hash(pattern):
                raise ValueError("Pattern hash doesn't match hash in compiled replace!")
            call = repl
        else:
            raise TypeError("Not a valid type!")
    else:
        raise TypeError("Pattern must be a compiled regular expression!")
    return call


def purge() -> None:
    """Purge caches."""

    _purge_cache()
    _regex.purge()


def expand(m: Match[AnyStr] | None, repl: ReplaceTemplate[AnyStr] | AnyStr) -> AnyStr:
    """Expand the string using the replace pattern or function."""

    _assert_expandable(repl)
    return _apply_replace_backrefs(m, repl)


def expandf(m: Match[AnyStr] | None, repl: ReplaceTemplate[AnyStr] | AnyStr) -> AnyStr:
    """Expand the string using the format replace pattern or function."""

    _assert_expandable(repl, True)
    return _apply_replace_backrefs(m, repl, flags=FORMAT)


def prefixmatch(
    pattern: AnyStr | Pattern[AnyStr] | Bregex[AnyStr],
    string: AnyStr,
    flags: int = 0,
    *args: Any,
    **kwargs: Any
) -> Match[AnyStr] | None:
    """Wrapper for `match`."""

    return cast(
        'Match[AnyStr] | None',
        _regex.match(_apply_search_backrefs(pattern, flags), string, flags, *args, **kwargs)
    )


match = prefixmatch


def fullmatch(
    pattern: AnyStr | Pattern[AnyStr] | Bregex[AnyStr],
    string: AnyStr,
    flags: int = 0,
    *args: Any,
    **kwargs: Any
) -> Match[AnyStr] | None:
    """Wrapper for `fullmatch`."""

    return cast(
        'Match[AnyStr] | None',
        _regex.fullmatch(_apply_search_backrefs(pattern, flags), string, flags, *args, **kwargs)
    )


def search(
    pattern: AnyStr | Pattern[AnyStr] | Bregex[AnyStr],
    string: AnyStr,
    flags: int = 0,
    *args: Any,
    **kwargs: Any
) -> Match[AnyStr] | None:
    """Wrapper for `search`."""

    return cast(
        'Match[AnyStr] | None',
        _regex.search(_apply_search_backrefs(pattern, flags), string, flags, *args, **kwargs)
    )


def sub(
    pattern: AnyStr | Pattern[AnyStr] | Bregex[AnyStr],
    repl: AnyStr | Callable[..., AnyStr],
    string: AnyStr,
    count: int = 0,
    flags: int = 0,
    *args: Any,
    **kwargs: Any
) -> AnyStr:
    """Wrapper for `sub`."""

    is_replace = _is_replace(repl)
    is_string = isinstance(repl, (str, bytes))
    if is_replace and cast(ReplaceTemplate[AnyStr], repl).use_format:
        raise ValueError("Compiled replace cannot be a format object!")

    pattern = compile_search(pattern, flags)
    return cast(
        AnyStr,
        _regex.sub(
            pattern, (compile_replace(pattern, repl) if is_replace or is_string else repl), string,
            count,
            0,
            *args, **kwargs
        )
    )


def subf(
    pattern: AnyStr | Pattern[AnyStr] | Bregex[AnyStr],
    repl: AnyStr | Callable[..., AnyStr],
    string: AnyStr,
    count: int = 0,
    flags: int = 0,
    *args: Any,
    **kwargs: Any
) -> AnyStr:
    """Wrapper for `subf`."""

    is_replace = _is_replace(repl)
    is_string = isinstance(repl, (str, bytes))
    if is_replace and not cast(ReplaceTemplate[AnyStr], repl).use_format:
        raise ValueError("Compiled replace is not a format object!")

    pattern = compile_search(pattern, flags)
    rflags = FORMAT if is_string else 0
    return cast(
        AnyStr,
        _regex.sub(
            pattern, (compile_replace(pattern, repl, flags=rflags) if is_replace or is_string else repl), string,
            count,
            0,
            *args, **kwargs
        )
    )


def subn(
    pattern: AnyStr | Pattern[AnyStr] | Bregex[AnyStr],
    repl: AnyStr | Callable[..., AnyStr],
    string: AnyStr,
    count: int = 0,
    flags: int = 0,
    *args: Any,
    **kwargs: Any
) -> tuple[AnyStr, int]:
    """Wrapper for `subn`."""

    is_replace = _is_replace(repl)
    is_string = isinstance(repl, (str, bytes))
    if is_replace and cast(ReplaceTemplate[AnyStr], repl).use_format:
        raise ValueError("Compiled replace cannot be a format object!")

    pattern = compile_search(pattern, flags)
    return cast(
        'tuple[AnyStr, int]',
        _regex.subn(
            pattern, (compile_replace(pattern, repl) if is_replace or is_string else repl), string,
            count,
            0,
            *args, **kwargs
        )
    )


def subfn(
    pattern: AnyStr | Pattern[AnyStr] | Bregex[AnyStr],
    repl: AnyStr | Callable[..., AnyStr],
    string: AnyStr,
    count: int = 0,
    flags: int = 0,
    *args: Any,
    **kwargs: Any
) -> tuple[AnyStr, int]:
    """Wrapper for `subfn`."""

    is_replace = _is_replace(repl)
    is_string = isinstance(repl, (str, bytes))
    if is_replace and not cast(ReplaceTemplate[AnyStr], repl).use_format:
        raise ValueError("Compiled replace is not a format object!")

    pattern = compile_search(pattern, flags)
    rflags = FORMAT if is_string else 0
    return cast(
        'tuple[AnyStr, int]',
        _regex.subn(
            pattern, (compile_replace(pattern, repl, flags=rflags) if is_replace or is_string else repl), string,
            count,
            0,
            *args, **kwargs
        )
    )


def split(
    pattern: AnyStr | Pattern[AnyStr] | Bregex[AnyStr],
    string: AnyStr,
    maxsplit: int = 0,
    flags: int = 0,
    *args: Any,
    **kwargs: Any
) -> list[AnyStr]:
    """Wrapper for `split`."""

    return cast(
        'list[AnyStr]',
        _regex.split(_apply_search_backrefs(pattern, flags), string, maxsplit, flags, *args, **kwargs)
    )


def splititer(
    pattern: AnyStr | Pattern[AnyStr] | Bregex[AnyStr],
    string: AnyStr,
    maxsplit: int = 0,
    flags: int = 0,
    *args: Any,
    **kwargs: Any
) -> Iterator[AnyStr]:
    """Wrapper for `splititer`."""

    return cast(
        Iterator[AnyStr],
        _regex.splititer(_apply_search_backrefs(pattern, flags), string, maxsplit, flags, *args, **kwargs)
    )


def findall(
    pattern: AnyStr | Pattern[AnyStr] | Bregex[AnyStr],
    string: AnyStr,
    flags: int = 0,
    *args: Any,
    **kwargs: Any
) -> list[AnyStr] | list[tuple[AnyStr, ...]]:
    """Wrapper for `findall`."""

    return cast(
        'list[AnyStr] | list[tuple[AnyStr, ...]]',
        _regex.findall(_apply_search_backrefs(pattern, flags), string, flags, *args, **kwargs)
    )


def finditer(
    pattern: AnyStr | Pattern[AnyStr] | Bregex[AnyStr],
    string: AnyStr,
    flags: int = 0,
    *args: Any,
    **kwargs: Any
) -> Iterator[Match[AnyStr]]:
    """Wrapper for `finditer`."""

    return cast(
        Iterator[Match[AnyStr]],
        _regex.finditer(_apply_search_backrefs(pattern, flags), string, *args, **kwargs)
    )


def _pickle(p):  # type: ignore[no-untyped-def]
    return Bregex, (p._pattern, p.auto_compile)


_copyreg.pickle(Bregex, _pickle)
