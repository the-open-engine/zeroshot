r"""
Backrefs re.

Add the ability to use the following backrefs with re:

 - `\l`                                                          - Lowercase character class (search)
 - `\c`                                                          - Uppercase character class (search)
 - `\L`                                                          - Inverse of lowercase character class (search)
 - `\C`                                                          - Inverse of uppercase character class (search)
 - `\Q` and `\Q...\E`                                            - Escape/quote chars (search)
 - `\c` and `\C...\E`                                            - Uppercase char or chars (replace)
 - `\l` and `\L...\E`                                            - Lowercase char or chars (replace)
 - `[:ascii:]`                                                   - Posix style classes (search)
 - `[:^ascii:]`                                                  - Inverse Posix style classes (search)
 - `\pL`, `\p{Lu}`, \p{Letter}, `\p{gc=Uppercase_Letter}`        - Unicode properties (search Unicode)
 - `\PL`, `\P{Lu}`, `\p{^Lu}`                                    - Inverse Unicode properties (search Unicode)
 - `\N{Black Club Suit}`                                         - Unicode character by name (search & replace)
 - `\u0000` and `\U00000000`                                     - Unicode characters (replace)
 - `\m`                                                          - Starting word boundary (search)
 - `\M`                                                          - Ending word boundary (search)
 - `\R`                                                          - Generic line breaks (search)
 - `\X`                                                          - Simplified grapheme clusters (search)

Licensed under MIT
Copyright (c) 2011 - 2020 Isaac Muse <isaacmuse@gmail.com>
"""
from __future__ import annotations
import re as _re
import copyreg as _copyreg
from functools import lru_cache as _lru_cache
from . import util as _util
from .util import PatternError
from . import _bre_parse
from ._bre_parse import ReplaceTemplate
from typing import AnyStr, Pattern, Match, Callable, Any, Generic, Mapping, Iterator, cast

__all__ = (
    "expand", "expandf", "search", "match", "fullmatch", "split", "findall", "finditer", "sub", "subf",
    "subn", "subfn", "purge", "escape", "DEBUG", "I", "IGNORECASE", "L", "LOCALE", "M", "MULTILINE",
    "S", "DOTALL", "U", "UNICODE", "X", "VERBOSE", "compile", "compile_search", "compile_replace", "Bre",
    "ReplaceTemplate", "A", "ASCII", "PatternError"
)

# Expose some common re flags and methods to
# save having to import re and backrefs libraries
DEBUG = _re.DEBUG
I = _re.I
IGNORECASE = _re.IGNORECASE
L = _re.L
LOCALE = _re.LOCALE
M = _re.M
MULTILINE = _re.MULTILINE
S = _re.S
DOTALL = _re.DOTALL
U = _re.U
UNICODE = _re.UNICODE
X = _re.X
VERBOSE = _re.VERBOSE
A = _re.A
ASCII = _re.ASCII
escape = _re.escape

# Replace flags
FORMAT = 1

# Maximum size of the cache.
_MAXCACHE = 500

_RE_TYPE = type(_re.compile('', 0))


@_lru_cache(maxsize=_MAXCACHE)
def _cached_search_compile(
    pattern: AnyStr,
    re_verbose: bool,
    re_unicode: bool,
    pattern_type: type[AnyStr]
) -> AnyStr:
    """Cached search compile."""

    return _bre_parse._SearchParser(pattern, re_verbose, re_unicode).parse()


@_lru_cache(maxsize=_MAXCACHE)
def _cached_replace_compile(
    pattern: Pattern[AnyStr],
    repl: AnyStr,
    flags: int,
    pattern_type: type[AnyStr]
) -> ReplaceTemplate[AnyStr]:
    """Cached replace compile."""

    return _bre_parse._ReplaceParser(pattern, repl, bool(flags & FORMAT)).parse()


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
    return _bre_parse._ReplaceParser(m.re, repl, bool(flags & FORMAT)).parse().expand(m)


def _apply_search_backrefs(
    pattern: AnyStr | Pattern[AnyStr] | Bre[AnyStr],
    flags: int = 0
) -> AnyStr | Pattern[AnyStr]:
    """Apply the search backrefs to the search pattern."""

    if isinstance(pattern, (str, bytes)):
        re_verbose = bool(VERBOSE & flags)
        re_unicode = None
        if bool((ASCII | LOCALE) & flags):
            re_unicode = False
        elif bool(UNICODE & flags):
            re_unicode = True
        if not (flags & DEBUG):
            p = _cached_search_compile(
                pattern, re_verbose, re_unicode, type(pattern)
            )  # type: AnyStr | Pattern[AnyStr]
        else:  # pragma: no cover
            p = _bre_parse._SearchParser(pattern, re_verbose, re_unicode).parse()
    elif isinstance(pattern, Bre):
        if flags:
            raise ValueError("Cannot process flags argument with a compiled pattern")
        p = pattern._pattern
    elif isinstance(pattern, _RE_TYPE):
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
class Bre(_util.Immutable, Generic[AnyStr]):
    """Bre object."""

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

        return self._pattern.pattern

    @property
    def flags(self) -> int:
        """Return flags."""

        return self._pattern.flags

    @property
    def groupindex(self) -> Mapping[str, int]:
        """Return group index."""

        return self._pattern.groupindex

    @property
    def groups(self) -> tuple[AnyStr | None, ...]:
        """Return groups."""

        return cast('tuple[AnyStr | None, ...]', self._pattern.groups)

    @property
    def scanner(self) -> Any:
        """Return scanner."""

        return self._pattern.scanner  # type: ignore[attr-defined]

    def __hash__(self) -> int:
        """Hash."""

        return self._hash

    def __eq__(self, other: Any) -> bool:
        """Equal."""

        return (
            isinstance(other, Bre) and
            self._pattern == other._pattern and
            self.auto_compile == other.auto_compile
        )

    def __ne__(self, other: Any) -> bool:
        """Equal."""

        return (
            not isinstance(other, Bre) or
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

        return self._pattern.match(string, *args, **kwargs)

    def fullmatch(
        self,
        string: AnyStr,
        *args: Any,
        **kwargs: Any
    ) -> Match[AnyStr] | None:
        """Apply `fullmatch`."""

        return self._pattern.fullmatch(string, *args, **kwargs)

    def split(
        self,
        string: AnyStr,
        *args: Any,
        **kwargs: Any
    ) -> list[AnyStr]:
        """Apply `split`."""

        return self._pattern.split(string, *args, **kwargs)

    def findall(
        self,
        string: AnyStr,
        *args: Any,
        **kwargs: Any
    ) -> list[AnyStr] | list[tuple[AnyStr, ...]]:
        """Apply `findall`."""

        return self._pattern.findall(string, *args, **kwargs)

    def finditer(
        self,
        string: AnyStr,
        *args: Any,
        **kwargs: Any
    ) -> Iterator[Match[AnyStr]]:
        """Apply `finditer`."""

        return self._pattern.finditer(string, *args, **kwargs)

    def sub(
        self,
        repl: AnyStr | Callable[..., AnyStr],
        string: AnyStr,
        *args: Any,
        **kwargs: Any
    ) -> AnyStr:
        """Apply `sub`."""

        return self._pattern.sub(self._auto_compile(repl), string, *args, **kwargs)

    def subf(  # noqa A002
        self,
        repl: AnyStr | Callable[..., AnyStr],
        string: AnyStr,
        *args: Any,
        **kwargs: Any
    ) -> AnyStr:
        """Apply `sub` with format style replace."""

        return self._pattern.sub(self._auto_compile(repl, True), string, *args, **kwargs)

    def subn(
        self,
        repl: AnyStr | Callable[..., AnyStr],
        string: AnyStr,
        *args: Any,
        **kwargs: Any
    ) -> tuple[AnyStr, int]:
        """Apply `subn` with format style replace."""

        return self._pattern.subn(self._auto_compile(repl), string, *args, **kwargs)

    def subfn(  # noqa A002
        self,
        repl: AnyStr | Callable[..., AnyStr],
        string: AnyStr,
        *args: Any,
        **kwargs: Any
    ) -> tuple[AnyStr, int]:
        """Apply `subn` after applying backrefs."""

        return self._pattern.subn(self._auto_compile(repl, True), string, *args, **kwargs)


def compile(  # noqa A001
    pattern: AnyStr | Pattern[AnyStr] | Bre[AnyStr],
    flags: int = 0,
    auto_compile: bool | None = None
) -> Bre[AnyStr]:
    """Compile both the search or search and replace into one object."""

    if isinstance(pattern, Bre):
        if auto_compile is not None:
            raise ValueError("Cannot compile Bre with a different auto_compile!")
        elif flags != 0:
            raise ValueError("Cannot process flags argument with a compiled pattern")
        return pattern
    else:
        if auto_compile is None:
            auto_compile = True

        return Bre(compile_search(pattern, flags), auto_compile)


def compile_search(
    pattern: AnyStr | Pattern[AnyStr] | Bre[AnyStr],
    flags: int = 0
) -> Pattern[AnyStr]:
    """Compile with extended search references."""

    return _re.compile(_apply_search_backrefs(pattern, flags), flags)


def compile_replace(
    pattern: Pattern[AnyStr],
    repl: AnyStr | Callable[..., AnyStr],
    flags: int = 0
) -> Callable[..., AnyStr]:
    """Construct a method that can be used as a replace method for `sub`, `subn`, etc."""

    if pattern is not None and isinstance(pattern, _RE_TYPE):
        if isinstance(repl, (str, bytes)):
            if not (pattern.flags & DEBUG):
                call = _cached_replace_compile(pattern, repl, flags, type(repl))
            else:  # pragma: no cover
                call = _bre_parse._ReplaceParser(pattern, repl, bool(flags & FORMAT)).parse()
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
    _re.purge()


def expand(m: Match[AnyStr] | None, repl: ReplaceTemplate[AnyStr] | AnyStr) -> AnyStr:
    """Expand the string using the replace pattern or function."""

    _assert_expandable(repl)
    return _apply_replace_backrefs(m, repl)


def expandf(m: Match[AnyStr] | None, repl: ReplaceTemplate[AnyStr] | AnyStr) -> AnyStr:
    """Expand the string using the format replace pattern or function."""

    _assert_expandable(repl, True)
    return _apply_replace_backrefs(m, repl, flags=FORMAT)


def search(
    pattern: AnyStr | Pattern[AnyStr] | Bre[AnyStr],
    string: AnyStr,
    flags: int | _re.RegexFlag = 0,
    *args: Any,
    **kwargs: Any
) -> Match[AnyStr] | None:
    """Apply `search` after applying backrefs."""

    return _re.search(_apply_search_backrefs(pattern, flags), string, flags, *args, **kwargs)


def prefixmatch(
    pattern: AnyStr | Pattern[AnyStr] | Bre[AnyStr],
    string: AnyStr,
    flags: int | _re.RegexFlag = 0,
    **kwargs: Any
) -> Match[AnyStr] | None:
    """Apply `match` after applying backrefs."""

    return _re.match(_apply_search_backrefs(pattern, flags), string, flags=flags, **kwargs)


match = prefixmatch


def fullmatch(
    pattern: AnyStr | Pattern[AnyStr] | Bre[AnyStr],
    string: AnyStr,
    flags: int | _re.RegexFlag = 0,
    **kwargs: Any
) -> Match[AnyStr] | None:
    """Apply `fullmatch` after applying backrefs."""

    return _re.fullmatch(_apply_search_backrefs(pattern, flags), string, flags=flags, **kwargs)


def split(
    pattern: AnyStr | Pattern[AnyStr] | Bre[AnyStr],
    string: AnyStr,
    maxsplit: int = 0,
    flags: int | _re.RegexFlag = 0,
    **kwargs: Any
) -> list[AnyStr]:
    """Apply `split` after applying backrefs."""

    return _re.split(
        _apply_search_backrefs(pattern, flags),
        string,
        maxsplit=maxsplit,
        flags=flags,
        **kwargs
    )


def findall(
    pattern: AnyStr | Pattern[AnyStr] | Bre[AnyStr],
    string: AnyStr,
    flags: int | _re.RegexFlag = 0,
    **kwargs: Any
) -> list[AnyStr] | list[tuple[AnyStr, ...]]:
    """Apply `findall` after applying backrefs."""

    return _re.findall(_apply_search_backrefs(pattern, flags), string, flags=flags, **kwargs)


def finditer(
    pattern: AnyStr | Pattern[AnyStr] | Bre[AnyStr],
    string: AnyStr,
    flags: int | _re.RegexFlag = 0,
    **kwargs: Any
) -> Iterator[Match[AnyStr]]:
    """Apply `finditer` after applying backrefs."""

    return _re.finditer(_apply_search_backrefs(pattern, flags), string, flags=flags, **kwargs)


def sub(
    pattern: AnyStr | Pattern[AnyStr] | Bre[AnyStr],
    repl: AnyStr | Callable[..., AnyStr],
    string: AnyStr,
    count: int = 0,
    flags: int | _re.RegexFlag = 0,
    **kwargs: Any
) -> AnyStr:
    """Apply `sub` after applying backrefs."""

    is_replace = _is_replace(repl)
    is_string = isinstance(repl, (str, bytes))
    if is_replace and cast(ReplaceTemplate[AnyStr], repl).use_format:
        raise ValueError("Compiled replace cannot be a format object!")

    pattern = compile_search(pattern, flags)
    return _re.sub(
        pattern,
        (compile_replace(pattern, repl) if is_replace or is_string else repl),
        string,
        count=count,
        flags=0,
        **kwargs
    )


def subf(
    pattern: AnyStr | Pattern[AnyStr] | Bre[AnyStr],
    repl: AnyStr | Callable[..., AnyStr],
    string: AnyStr,
    count: int = 0,
    flags: int | _re.RegexFlag = 0,
    **kwargs: Any
) -> AnyStr:
    """Apply `sub` with format style replace."""

    is_replace = _is_replace(repl)
    is_string = isinstance(repl, (str, bytes))
    if is_replace and not cast(ReplaceTemplate[AnyStr], repl).use_format:
        raise ValueError("Compiled replace is not a format object!")

    pattern = compile_search(pattern, flags)
    rflags = FORMAT if is_string else 0
    return _re.sub(
        pattern,
        (compile_replace(pattern, repl, flags=rflags) if is_replace or is_string else repl),
        string,
        count=count,
        flags=0,
        **kwargs
    )


def subn(
    pattern: AnyStr | Pattern[AnyStr] | Bre[AnyStr],
    repl: AnyStr | Callable[..., AnyStr],
    string: AnyStr,
    count: int = 0,
    flags: int | _re.RegexFlag = 0,
    **kwargs: Any
) -> tuple[AnyStr, int]:
    """Apply `subn` with format style replace."""

    is_replace = _is_replace(repl)
    is_string = isinstance(repl, (str, bytes))
    if is_replace and cast(ReplaceTemplate[AnyStr], repl).use_format:
        raise ValueError("Compiled replace cannot be a format object!")

    pattern = compile_search(pattern, flags)
    return _re.subn(
        pattern,
        (compile_replace(pattern, repl) if is_replace or is_string else repl),
        string,
        count=count,
        flags=0,
        **kwargs
    )


def subfn(
    pattern: AnyStr | Pattern[AnyStr] | Bre[AnyStr],
    repl: AnyStr | Callable[..., AnyStr],
    string: AnyStr,
    count: int = 0,
    flags: int | _re.RegexFlag = 0,
    **kwargs: Any
) -> tuple[AnyStr, int]:
    """Apply `subn` after applying backrefs."""

    is_replace = _is_replace(repl)
    is_string = isinstance(repl, (str, bytes))
    if is_replace and not cast(ReplaceTemplate[AnyStr], repl).use_format:
        raise ValueError("Compiled replace is not a format object!")

    pattern = compile_search(pattern, flags)
    rflags = FORMAT if is_string else 0
    return _re.subn(
        pattern,
        (compile_replace(pattern, repl, flags=rflags) if is_replace or is_string else repl),
        string,
        count=count,
        flags=0,
        **kwargs
    )


def _pickle(p):  # type: ignore[no-untyped-def]
    return Bre, (p._pattern, p.auto_compile)


_copyreg.pickle(Bre, _pickle)
