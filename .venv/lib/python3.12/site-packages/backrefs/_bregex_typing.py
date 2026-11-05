"""Typing objects for the Regex library."""
import regex  # type: ignore[import]
from typing import _alias  # type: ignore[attr-defined]

Pattern = _alias(type(regex.compile('')), 1)
Match = _alias(type(regex.compile('').match('')), 1)
