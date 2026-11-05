# Backlinks module.

from __future__ import annotations

import logging
from dataclasses import dataclass
from typing import TYPE_CHECKING, ClassVar

from markdown.core import Markdown
from markdown.treeprocessors import Treeprocessor

if TYPE_CHECKING:
    from xml.etree.ElementTree import Element

    from markdown import Markdown

    from mkdocs_autorefs._internal.plugin import AutorefsPlugin

try:
    from mkdocs.plugins import get_plugin_logger

    _log = get_plugin_logger(__name__)
except ImportError:
    # TODO: remove once support for MkDocs <1.5 is dropped
    _log = logging.getLogger(f"mkdocs.plugins.{__name__}")  # type: ignore[assignment]


@dataclass(frozen=True, order=True)
class BacklinkCrumb:  # noqa: PLW1641
    """A navigation breadcrumb for a backlink."""

    title: str
    """The title of the breadcrumb."""
    url: str
    """The URL of the breadcrumb."""
    parent: BacklinkCrumb | None = None
    """The parent breadcrumb."""

    def __eq__(self, value: object) -> bool:
        """Compare URLs for equality."""
        if isinstance(value, BacklinkCrumb):
            return self.url == value.url
        return False


@dataclass(eq=True, frozen=True, order=True)
class Backlink:
    """A backlink (list of breadcrumbs)."""

    crumbs: tuple[BacklinkCrumb, ...]
    """The list of breadcrumbs."""


class BacklinksTreeProcessor(Treeprocessor):
    """Enhance autorefs with `backlink-type` and `backlink-anchor` attributes.

    These attributes are then used later to register backlinks.
    """

    name: str = "mkdocs-autorefs-backlinks"
    """The name of the tree processor."""
    initial_id: str | None = None
    """The initial heading ID."""

    _htags: ClassVar[set[str]] = {"h1", "h2", "h3", "h4", "h5", "h6"}

    def __init__(self, plugin: AutorefsPlugin, md: Markdown | None = None) -> None:
        """Initialize the tree processor.

        Parameters:
            plugin: A reference to the autorefs plugin, to use its `register_anchor` method.
        """
        super().__init__(md)
        self._plugin = plugin
        self._last_heading_id: str | None = None

    def run(self, root: Element) -> None:
        """Run the tree processor.

        Arguments:
            root: The root element of the document.
        """
        if self._plugin.current_page is not None:
            self._last_heading_id = self.initial_id
            self._enhance_autorefs(root)

    def _enhance_autorefs(self, parent: Element) -> None:
        for el in parent:
            if el.tag in self._htags:
                self._last_heading_id = el.get("id")
            elif el.tag == "autoref":
                if "backlink-type" not in el.attrib:
                    el.set("backlink-type", "referenced-by")
                if "backlink-anchor" not in el.attrib and self._last_heading_id:
                    el.set("backlink-anchor", self._last_heading_id)
            else:
                self._enhance_autorefs(el)
