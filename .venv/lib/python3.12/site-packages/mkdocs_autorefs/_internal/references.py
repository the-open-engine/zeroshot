# Cross-references module.

from __future__ import annotations

import logging
import re
import warnings
from abc import ABC, abstractmethod
from dataclasses import dataclass
from functools import lru_cache
from html import escape, unescape
from html.parser import HTMLParser
from io import StringIO
from typing import TYPE_CHECKING, Any, Callable, ClassVar, Literal
from urllib.parse import urlsplit
from xml.etree.ElementTree import Element

from markdown.core import Markdown
from markdown.extensions import Extension
from markdown.extensions.toc import slugify
from markdown.inlinepatterns import REFERENCE_RE, ReferenceInlineProcessor
from markdown.treeprocessors import Treeprocessor
from markdown.util import HTML_PLACEHOLDER_RE, INLINE_PLACEHOLDER_RE
from markupsafe import Markup

from mkdocs_autorefs._internal.backlinks import BacklinksTreeProcessor

if TYPE_CHECKING:
    from collections.abc import Iterable
    from pathlib import Path
    from re import Match

    from markdown import Markdown

    from mkdocs_autorefs._internal.plugin import AutorefsPlugin

try:
    from mkdocs.plugins import get_plugin_logger

    _log = get_plugin_logger(__name__)
except ImportError:
    # TODO: remove once support for MkDocs <1.5 is dropped
    _log = logging.getLogger(f"mkdocs.plugins.{__name__}")  # type: ignore[assignment]


AUTOREF_RE = re.compile(r"<autoref (?P<attrs>.*?)>(?P<title>.*?)</autoref>", flags=re.DOTALL)
"""The autoref HTML tag regular expression.

A regular expression to match mkdocs-autorefs' special reference markers
in the [`on_env` hook][mkdocs_autorefs.AutorefsPlugin.on_env].
"""


class AutorefsHookInterface(ABC):
    """An interface for hooking into how AutoRef handles inline references."""

    @dataclass
    class Context:
        """The context around an auto-reference."""

        domain: str
        """A domain like `py` or `js`."""
        role: str
        """A role like `class` or `function`."""
        origin: str
        """The origin of an autoref (an object identifier)."""
        filepath: str | Path
        """The path to the file containing the autoref."""
        lineno: int
        """The line number in the file containing the autoref."""

        def as_dict(self) -> dict[str, str]:
            """Convert the context to a dictionary of HTML attributes."""
            return {
                "domain": self.domain,
                "role": self.role,
                "origin": self.origin,
                "filepath": str(self.filepath),
                "lineno": str(self.lineno),
            }

    @abstractmethod
    def expand_identifier(self, identifier: str) -> str:
        """Expand an identifier in a given context.

        Parameters:
            identifier: The identifier to expand.

        Returns:
            The expanded identifier.
        """
        raise NotImplementedError

    @abstractmethod
    def get_context(self) -> AutorefsHookInterface.Context:
        """Get the current context.

        Returns:
            The current context.
        """
        raise NotImplementedError


class AutorefsInlineProcessor(ReferenceInlineProcessor):
    """A Markdown extension to handle inline references."""

    name: str = "mkdocs-autorefs"
    """The name of the inline processor."""

    hook: AutorefsHookInterface | None = None
    """The hook to use for expanding identifiers or adding context to autorefs."""

    def __init__(self, *args: Any, **kwargs: Any) -> None:
        super().__init__(REFERENCE_RE, *args, **kwargs)

    # Code based on
    # https://github.com/Python-Markdown/markdown/blob/8e7528fa5c98bf4652deb13206d6e6241d61630b/markdown/inlinepatterns.py#L780
    def handleMatch(self, m: Match[str], data: str) -> tuple[Element | None, int | None, int | None]:  # type: ignore[override]  # noqa: N802
        """Handle an element that matched.

        Arguments:
            m: The match object.
            data: The matched data.

        Returns:
            A new element or a tuple.
        """
        text, index, handled = self.getText(data, m.end(0))
        if not handled:
            return None, None, None

        identifier, slug, end, handled = self._eval_id(data, index, text)
        if not handled or identifier is None:
            return None, None, None

        if slug is None and re.search(r"[\x00-\x1f]", identifier):
            # Do nothing if the matched reference still contains control characters (from 0 to 31 included)
            # that weren't unstashed when trying to compute a slug of the title.
            return None, m.start(0), end

        return self._make_tag(identifier, text, slug=slug), m.start(0), end

    def _unstash(self, identifier: str) -> str:
        stashed_nodes: dict[str, Element | str] = self.md.treeprocessors["inline"].stashed_nodes  # type: ignore[attr-defined]

        def _repl(match: Match) -> str:
            el = stashed_nodes.get(match[1])
            if isinstance(el, Element):
                return f"`{''.join(el.itertext())}`"
            if el == "\x0296\x03":
                return "`"
            return str(el)

        return INLINE_PLACEHOLDER_RE.sub(_repl, identifier)

    def _eval_id(self, data: str, index: int, text: str) -> tuple[str | None, str | None, int, bool]:
        """Evaluate the id portion of `[ref][id]`.

        If `[ref][]` use `[ref]`.

        Arguments:
            data: The data to evaluate.
            index: The starting position.
            text: The text to use when no identifier.

        Returns:
            A tuple containing the identifier, its optional slug, its end position, and whether it matched.
        """
        m = self.RE_LINK.match(data, pos=index)
        if not m:
            return None, None, index, False

        if identifier := m.group(1):
            # An identifier was provided, match it exactly (later).
            slug = None
        else:
            # Only a title was provided, use it as identifier.
            identifier = text

            # Catch single stash entries, like the result of [`Foo`][].
            if match := INLINE_PLACEHOLDER_RE.fullmatch(identifier):
                stashed_nodes: dict[str, Element | str] = self.md.treeprocessors["inline"].stashed_nodes  # type: ignore[attr-defined]
                el = stashed_nodes.get(match[1])
                if isinstance(el, Element) and el.tag == "code":
                    # The title was wrapped in backticks, we only keep the content,
                    # and tell autorefs to match the identifier exactly.
                    identifier = "".join(el.itertext())
                    slug = None
                    # Special case: allow pymdownx.inlinehilite raw <code> snippets but strip them back to unhighlighted.
                    if match := HTML_PLACEHOLDER_RE.fullmatch(identifier):
                        stash_index = int(match.group(1))
                        html = self.md.htmlStash.rawHtmlBlocks[stash_index]
                        identifier = Markup(html).striptags()  # noqa: S704
                        self.md.htmlStash.rawHtmlBlocks[stash_index] = escape(identifier)

            # In any other case, unstash the title and slugify it.
            # Examples: ``[`Foo` and `Bar`]``, `[The *Foo*][]`.
            else:
                identifier = self._unstash(identifier)
                slug = slugify(identifier, separator="-")

        end = m.end(0)
        return identifier, slug, end, True

    def _make_tag(self, identifier: str, text: str, *, slug: str | None = None) -> Element:
        """Create a tag that can be matched by `AUTO_REF_RE`.

        Arguments:
            identifier: The identifier to use in the HTML property.
            text: The text to use in the HTML tag.

        Returns:
            A new element.
        """
        el = Element("autoref")
        if self.hook:
            identifier = self.hook.expand_identifier(identifier)
            el.attrib.update(self.hook.get_context().as_dict())
        el.set("identifier", identifier)
        el.text = text
        if slug:
            el.attrib["slug"] = slug
        return el


class AnchorScannerTreeProcessor(Treeprocessor):
    """Tree processor to scan and register HTML anchors."""

    name: str = "mkdocs-autorefs-anchors-scanner"
    """The name of the tree processor."""

    _htags: ClassVar[set[str]] = {"h1", "h2", "h3", "h4", "h5", "h6"}

    def __init__(self, plugin: AutorefsPlugin, md: Markdown | None = None) -> None:
        """Initialize the tree processor.

        Parameters:
            plugin: A reference to the autorefs plugin, to use its `register_anchor` method.
        """
        super().__init__(md)
        self._plugin = plugin

    def run(self, root: Element) -> None:
        """Run the tree processor.

        Arguments:
            root: The root element of the tree.
        """
        if self._plugin.current_page is not None:
            pending_anchors = _PendingAnchors(self._plugin)
            self._scan_anchors(root, pending_anchors)
            pending_anchors.flush()

    def _scan_anchors(self, parent: Element, pending_anchors: _PendingAnchors, last_heading: str | None = None) -> None:
        for el in parent:
            if el.tag == "a":
                # We found an anchor. Record its id if it has one.
                if anchor_id := el.get("id"):
                    pending_anchors.append(anchor_id)
                # If the element has text or a link, it's not an alias.
                # Non-whitespace text after the element interrupts the chain, aliases can't apply.
                if el.text or el.get("href") or (el.tail and el.tail.strip()):
                    pending_anchors.flush(title=last_heading)

            elif el.tag == "p":
                # A `p` tag is a no-op for our purposes, just recurse into it in the context
                # of the current collection of anchors.
                self._scan_anchors(el, pending_anchors, last_heading)
                # Non-whitespace text after the element interrupts the chain, aliases can't apply.
                if el.tail and el.tail.strip():
                    pending_anchors.flush()

            elif el.tag in self._htags:
                # If the element is a heading, that turns the pending anchors into aliases.
                last_heading = el.text
                pending_anchors.flush(el.get("id"), title=last_heading)

            else:
                # But if it's some other interruption, flush anchors anyway as non-aliases.
                pending_anchors.flush(title=last_heading)
                # Recurse into sub-elements, in a *separate* context.
                self.run(el)


class HeadingScannerTreeProcessor(Treeprocessor):
    """Tree processor to scan and register HTML headings."""

    name: str = "mkdocs-autorefs-headings-scanner"
    """The name of the tree processor."""

    _htags: ClassVar[set[str]] = {"h1", "h2", "h3", "h4", "h5", "h6"}

    def __init__(self, plugin: AutorefsPlugin, md: Markdown | None = None) -> None:
        """Initialize the tree processor.

        Parameters:
            plugin: A reference to the autorefs plugin, to use its `register_anchor` method.
        """
        super().__init__(md)
        self._plugin = plugin

    def run(self, root: Element) -> None:
        """Run the tree processor.

        Arguments:
            root: The root element of the tree.
        """
        if self._plugin.current_page is not None:
            self._scan_headings(root)

    def _scan_headings(self, parent: Element) -> None:
        for el in parent:
            if el.tag in self._htags:
                if h_id := el.get("id"):
                    self._plugin.register_anchor(
                        self._plugin.current_page,  # type: ignore[arg-type]
                        h_id,
                        title=el.text,
                    )
            else:
                self._scan_headings(el)


class AutorefsExtension(Extension):
    """Markdown extension that transforms unresolved references into auto-references.

    Auto-references are then resolved later by the MkDocs plugin.

    This extension also scans Markdown anchors (`[](){#some-id}`)
    to register them with the MkDocs plugin.
    """

    def __init__(
        self,
        plugin: AutorefsPlugin | None = None,
        **kwargs: Any,
    ) -> None:
        """Initialize the Markdown extension.

        Parameters:
            plugin: An optional reference to the autorefs plugin (to pass it to the anchor scanner tree processor).
            **kwargs: Keyword arguments passed to the [base constructor][markdown.Extension].
        """
        super().__init__(**kwargs)
        self.plugin = plugin
        """A reference to the autorefs plugin."""

    def extendMarkdown(self, md: Markdown) -> None:  # noqa: N802 (casing: parent method's name)
        """Register the extension.

        Add an instance of our [`AutorefsInlineProcessor`][mkdocs_autorefs.AutorefsInlineProcessor] to the Markdown parser.
        Also optionally add an instance of our [`AnchorScannerTreeProcessor`][mkdocs_autorefs.AnchorScannerTreeProcessor]
        and [`BacklinksTreeProcessor`][mkdocs_autorefs.BacklinksTreeProcessor] to the Markdown parser
        if a reference to the autorefs plugin was passed to this extension.

        Arguments:
            md: A `markdown.Markdown` instance.
        """
        md.inlinePatterns.register(
            AutorefsInlineProcessor(md),
            AutorefsInlineProcessor.name,
            priority=168,  # Right after markdown.inlinepatterns.ReferenceInlineProcessor
        )
        if self.plugin is not None:
            # Scan headings to register them.
            if self.plugin.scan_toc:
                md.treeprocessors.register(
                    HeadingScannerTreeProcessor(self.plugin, md),
                    HeadingScannerTreeProcessor.name,
                    priority=0,
                )
            # Markdown anchors require the `attr_list` extension.
            if self.plugin.scan_toc and "attr_list" in md.treeprocessors:
                _log_enabling_markdown_anchors()
                md.treeprocessors.register(
                    AnchorScannerTreeProcessor(self.plugin, md),
                    AnchorScannerTreeProcessor.name,
                    priority=0,
                )
            # Backlinks require IDs on headings, which are either set by `toc`,
            # or manually by the user with `attr_list`.
            if self.plugin.record_backlinks and ("attr_list" in md.treeprocessors or "toc" in md.treeprocessors):
                _log_enabling_backlinks()
                md.treeprocessors.register(
                    BacklinksTreeProcessor(self.plugin, md),
                    BacklinksTreeProcessor.name,
                    priority=0,
                )


class _PendingAnchors:
    """A collection of HTML anchors that may or may not become aliased to an upcoming heading."""

    def __init__(self, plugin: AutorefsPlugin):
        self.plugin = plugin
        self.anchors: list[str] = []

    def append(self, anchor: str) -> None:
        self.anchors.append(anchor)

    def flush(self, alias_to: str | None = None, title: str | None = None) -> None:
        if page := self.plugin.current_page:
            for anchor in self.anchors:
                self.plugin.register_anchor(page, anchor, alias_to, title=title, primary=True)
            self.anchors.clear()


class _AutorefsAttrs(dict):
    _handled_attrs: ClassVar[set[str]] = {
        "identifier",
        "optional",
        "hover",  # TODO: Remove at some point.
        "class",
        "domain",
        "role",
        "origin",
        "filepath",
        "lineno",
        "slug",
        "backlink-type",
        "backlink-anchor",
    }

    @property
    def context(self) -> AutorefsHookInterface.Context | None:
        try:
            return AutorefsHookInterface.Context(
                domain=self["domain"],
                role=self["role"],
                origin=self["origin"],
                filepath=self["filepath"],
                lineno=int(self["lineno"]),
            )
        except KeyError:
            return None

    @property
    def remaining(self) -> str:
        return " ".join(k if v is None else f'{k}="{v}"' for k, v in self.items() if k not in self._handled_attrs)


class _HTMLAttrsParser(HTMLParser):
    def __init__(self):
        super().__init__()
        self.attrs = {}

    def parse(self, html: str) -> _AutorefsAttrs:
        self.reset()
        self.attrs.clear()
        self.feed(html)
        return _AutorefsAttrs(self.attrs)

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:  # noqa: ARG002
        self.attrs.update(attrs)


class _HTMLTagStripper(HTMLParser):
    def __init__(self) -> None:
        super().__init__()
        self.text = StringIO()

    def strip(self, html: str) -> str:
        self.reset()
        self.text = StringIO()
        self.feed(html)
        return self.text.getvalue()

    def handle_data(self, data: str) -> None:
        self.text.write(data)


def relative_url(url_a: str, url_b: str) -> str:
    """Compute the relative path from URL A to URL B.

    Arguments:
        url_a: URL A.
        url_b: URL B.

    Returns:
        The relative URL to go from A to B.
    """
    parts_a = url_a.split("/")
    url_b, *rest = url_b.split("#", 1)
    anchor = rest[0] if rest else ""
    parts_b = url_b.split("/")

    # Remove common left parts.
    while parts_a and parts_b and parts_a[0] == parts_b[0]:
        parts_a.pop(0)
        parts_b.pop(0)

    # Go up as many times as remaining a parts' depth.
    levels = len(parts_a) - 1
    parts_relative = [".."] * levels + parts_b
    relative = "/".join(parts_relative)
    return f"{relative}#{anchor}"


def fix_ref(
    url_mapper: Callable[[str], tuple[str, str | None]],
    unmapped: list[tuple[str, AutorefsHookInterface.Context | None]],
    record_backlink: Callable[[str, str, str], None] | None = None,
    *,
    link_titles: bool | Literal["external"] = True,
    strip_title_tags: bool = False,
) -> Callable:
    """Return a `repl` function for [`re.sub`](https://docs.python.org/3/library/re.html#re.sub).

    In our context, we match Markdown references and replace them with HTML links.

    When the matched reference's identifier was not mapped to an URL, we append the identifier to the outer
    `unmapped` list. It generally means the user is trying to cross-reference an object that was not collected
    and rendered, making it impossible to link to it. We catch this exception in the caller to issue a warning.

    Arguments:
        url_mapper: A callable that gets an object's site URL by its identifier,
            such as [mkdocs_autorefs.AutorefsPlugin.get_item_url][].
        unmapped: A list to store unmapped identifiers.
        record_backlink: A callable to record backlinks.
        link_titles: How to set HTML titles on links. Always (`True`), never (`False`), or external-only (`"external"`).
        strip_title_tags: Whether to strip HTML tags from link titles.

    Returns:
        The actual function accepting a [`Match` object](https://docs.python.org/3/library/re.html#match-objects)
        and returning the replacement strings.
    """

    def inner(match: Match) -> str:
        title = match["title"]
        attrs = _html_attrs_parser.parse(f"<a {match['attrs']}>")
        identifier: str = attrs["identifier"]
        slug = attrs.get("slug", None)
        optional = "optional" in attrs

        identifiers = (identifier, slug) if slug else (identifier,)

        if (
            record_backlink
            and (backlink_type := attrs.get("backlink-type"))
            and (backlink_anchor := attrs.get("backlink-anchor"))
        ):
            record_backlink(identifier, backlink_type, backlink_anchor)

        try:
            url, original_title = _find_url(identifiers, url_mapper)
        except KeyError:
            if optional:
                _log.debug("Unresolved optional cross-reference: %s", identifier)
                return f'<span title="{identifier}">{title}</span>'
            unmapped.append((identifier, attrs.context))
            if title == identifier:
                return f"[{identifier}][]"
            if title == f"<code>{identifier}</code>" and not slug:
                return f"[<code>{identifier}</code>][]"
            return f"[{title}][{identifier}]"

        parsed = urlsplit(url)
        external = parsed.scheme or parsed.netloc

        classes = (attrs.get("class") or "").strip().split()
        classes = ["autorefs", "autorefs-external" if external else "autorefs-internal", *classes]
        class_attr = " ".join(classes)

        if remaining := attrs.remaining:
            remaining = f" {remaining}"

        title_attr = ""
        if link_titles is True or (link_titles == "external" and external):
            if optional:
                # The `optional` attribute is generally only added by mkdocstrings handlers,
                # for API objects, meaning we can and should append the full identifier.
                tooltip = _tooltip(identifier, original_title, strip_tags=strip_title_tags)
            else:
                # Autorefs without `optional` are generally user-written ones,
                # so we should only use the original title.
                tooltip = original_title or ""

            if tooltip and tooltip not in f"<code>{title}</code>":
                title_attr = f' title="{_html_tag_stripper.strip(tooltip) if strip_title_tags else escape(tooltip)}"'

        return f'<a class="{class_attr}"{title_attr} href="{escape(url)}"{remaining}>{title}</a>'

    return inner


def fix_refs(
    html: str,
    url_mapper: Callable[[str], tuple[str, str | None]],
    *,
    record_backlink: Callable[[str, str, str], None] | None = None,
    link_titles: bool | Literal["external"] = True,
    strip_title_tags: bool = False,
    # YORE: Bump 2: Remove line.
    _legacy_refs: bool = True,
) -> tuple[str, list[tuple[str, AutorefsHookInterface.Context | None]]]:
    """Fix all references in the given HTML text.

    Arguments:
        html: The text to fix.
        url_mapper: A callable that gets an object's site URL by its identifier,
            such as [mkdocs_autorefs.AutorefsPlugin.get_item_url][].
        record_backlink: A callable to record backlinks.
        link_titles: How to set HTML titles on links. Always (`True`), never (`False`), or external-only (`"external"`).
        strip_title_tags: Whether to strip HTML tags from link titles.

    Returns:
        The fixed HTML, and a list of unmapped identifiers (string and optional context).
    """
    unmapped: list[tuple[str, AutorefsHookInterface.Context | None]] = []
    html = AUTOREF_RE.sub(
        fix_ref(url_mapper, unmapped, record_backlink, link_titles=link_titles, strip_title_tags=strip_title_tags),
        html,
    )

    # YORE: Bump 2: Remove block.
    if _legacy_refs:
        html = AUTO_REF_RE.sub(_legacy_fix_ref(url_mapper, unmapped), html)

    return html, unmapped


def _find_url(
    identifiers: Iterable[str],
    url_mapper: Callable[[str], tuple[str, str | None]],
) -> tuple[str, str | None]:
    for identifier in identifiers:
        try:
            return url_mapper(identifier)
        except KeyError:
            pass
    raise KeyError(f"None of the identifiers {identifiers} were found")


def _tooltip(identifier: str, title: str | None, *, strip_tags: bool = False) -> str:
    if title:
        # Don't append identifier if it's already in the title.
        if identifier in title:
            return title
        # Append identifier (useful for API objects).
        if strip_tags:
            return f"{title} ({identifier})"
        return f"{title} (<code>{identifier}</code>)"
    # No title, just return the identifier.
    if strip_tags:
        return identifier
    return f"<code>{identifier}</code>"


@lru_cache
def _log_enabling_markdown_anchors() -> None:
    _log.debug("Enabling Markdown anchors feature")


@lru_cache
def _log_enabling_backlinks() -> None:
    _log.debug("Enabling backlinks feature")


_html_attrs_parser = _HTMLAttrsParser()
_html_tag_stripper = _HTMLTagStripper()


# YORE: Bump 2: Remove block.
_ATTR_VALUE = r'"[^"<>]+"|[^"<> ]+'  # Possibly with double quotes around
AUTO_REF_RE = re.compile(
    rf"<span data-(?P<kind>autorefs-(?:identifier|optional|optional-hover))=(?P<identifier>{_ATTR_VALUE})"
    rf"(?: class=(?P<class>{_ATTR_VALUE}))?(?P<attrs> [^<>]+)?>(?P<title>.*?)</span>",
    flags=re.DOTALL,
)
"""Deprecated. Use [`AUTOREF_RE`][mkdocs_autorefs.AUTOREF_RE] instead."""


# YORE: Bump 2: Remove block.
def __getattr__(name: str) -> Any:
    if name == "AutoRefInlineProcessor":
        warnings.warn("AutoRefInlineProcessor was renamed AutorefsInlineProcessor", DeprecationWarning, stacklevel=2)
        return AutorefsInlineProcessor
    raise AttributeError(f"module 'mkdocs_autorefs.references' has no attribute {name}")


# YORE: Bump 2: Remove block.
def _legacy_fix_ref(
    url_mapper: Callable[[str], tuple[str, str | None]],
    unmapped: list[tuple[str, AutorefsHookInterface.Context | None]],
) -> Callable:
    """Return a `repl` function for [`re.sub`](https://docs.python.org/3/library/re.html#re.sub).

    In our context, we match Markdown references and replace them with HTML links.

    When the matched reference's identifier was not mapped to an URL, we append the identifier to the outer
    `unmapped` list. It generally means the user is trying to cross-reference an object that was not collected
    and rendered, making it impossible to link to it. We catch this exception in the caller to issue a warning.

    Arguments:
        url_mapper: A callable that gets an object's site URL by its identifier,
            such as [mkdocs_autorefs.AutorefsPlugin.get_item_url][].
        unmapped: A list to store unmapped identifiers.

    Returns:
        The actual function accepting a [`Match` object](https://docs.python.org/3/library/re.html#match-objects)
        and returning the replacement strings.
    """

    def inner(match: Match) -> str:
        identifier = match["identifier"].strip('"')
        title = match["title"]
        kind = match["kind"]
        attrs = match["attrs"] or ""
        classes = (match["class"] or "").strip('"').split()

        try:
            url, _ = url_mapper(unescape(identifier))
        except KeyError:
            if kind == "autorefs-optional":
                return title
            if kind == "autorefs-optional-hover":
                return f'<span title="{identifier}">{title}</span>'
            unmapped.append((identifier, None))
            if title == identifier:
                return f"[{identifier}][]"
            return f"[{title}][{identifier}]"

        warnings.warn(
            "autorefs `span` elements are deprecated in favor of `autoref` elements: "
            f'`<span data-autorefs-identifier="{identifier}">...</span>` becomes `<autoref identifer="{identifier}">...</autoref>`',
            DeprecationWarning,
            stacklevel=1,
        )
        parsed = urlsplit(url)
        external = parsed.scheme or parsed.netloc
        classes = ["autorefs", "autorefs-external" if external else "autorefs-internal", *classes]
        class_attr = " ".join(classes)
        if kind == "autorefs-optional-hover":
            return f'<a class="{class_attr}" title="{identifier}" href="{escape(url)}"{attrs}>{title}</a>'
        return f'<a class="{class_attr}" href="{escape(url)}"{attrs}>{title}</a>'

    return inner
