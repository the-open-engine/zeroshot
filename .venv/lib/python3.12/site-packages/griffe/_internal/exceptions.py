# SPDX-License-Identifier: ISC

# Copyright (c) 2021, Timothée Mazzucotelli and contributors

# Permission to use, copy, modify, and/or distribute this software for any
# purpose with or without fee is hereby granted, provided that the above
# copyright notice and this permission notice appear in all copies.

# THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
# WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
# MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
# ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
# WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
# ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
# OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.

# This module contains all the exceptions specific to Griffe.

from __future__ import annotations

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from griffe._internal.models import Alias


class GriffeError(Exception):
    """The base exception for all Griffe errors."""


class LoadingError(GriffeError):
    """The base exception for all Griffe errors."""


class NameResolutionError(GriffeError):
    """Exception for names that cannot be resolved in a object scope."""


class UnhandledEditableModuleError(GriffeError):
    """Exception for unhandled editables modules, when searching modules."""


class UnimportableModuleError(GriffeError):
    """Exception for modules that cannot be imported."""


class AliasResolutionError(GriffeError):
    """Exception for alias that cannot be resolved."""

    def __init__(self, alias: Alias) -> None:
        """Initialize the exception.

        Parameters:
            alias: The alias that could not be resolved.
        """
        self.alias: Alias = alias
        """The alias that triggered the error."""

        self._detailed_message: str | None = None
        super().__init__(f"Could not resolve alias {alias.path} pointing at {self.alias.target_path}")

    def _format_message(self) -> str:
        message = super().__str__()
        try:
            filepath = self.alias.parent.relative_filepath  # ty:ignore[unresolved-attribute]
        except BuiltinModuleError:
            return message
        return message + f" (in {filepath}:{self.alias.alias_lineno})"

    def __str__(self) -> str:
        if self._detailed_message is None:
            self._detailed_message = self._format_message()
        return self._detailed_message


class CyclicAliasError(GriffeError):
    """Exception raised when a cycle is detected in aliases."""

    def __init__(self, chain: list[str]) -> None:
        """Initialize the exception.

        Parameters:
            chain: The cyclic chain of items (such as target path).
        """
        self.chain: list[str] = chain
        """The chain of aliases that created the cycle."""

        super().__init__("Cyclic aliases detected:\n  " + "\n  ".join(self.chain))


class LastNodeError(GriffeError):
    """Exception raised when trying to access a next or previous node."""


class RootNodeError(GriffeError):
    """Exception raised when trying to use siblings properties on a root node."""


class BuiltinModuleError(GriffeError):
    """Exception raised when trying to access the filepath of a builtin module."""


class ExtensionError(GriffeError):
    """Base class for errors raised by extensions."""


class ExtensionNotLoadedError(ExtensionError):
    """Exception raised when an extension could not be loaded."""


class GitError(GriffeError):
    """Exception raised for errors related to Git."""
