# This module contains the "mkdocs-autorefs" plugin.
#
# After each page is processed by the Markdown converter, this plugin stores absolute URLs of every HTML anchors
# it finds to later be able to fix unresolved references.
#
# Once every page has been rendered and all identifiers and their URLs collected,
# the plugin fixes unresolved references in the HTML content of the pages.

from __future__ import annotations

import contextlib
import functools
import logging
from collections import defaultdict
from pathlib import PurePosixPath as URL  # noqa: N814
from typing import TYPE_CHECKING, Any, Callable, Literal
from urllib.parse import urlsplit
from warnings import warn

from mkdocs.config.base import Config
from mkdocs.config.config_options import Choice, Type
from mkdocs.plugins import BasePlugin, event_priority
from mkdocs.structure.pages import Page

from mkdocs_autorefs._internal.backlinks import Backlink, BacklinkCrumb
from mkdocs_autorefs._internal.references import AutorefsExtension, fix_refs, relative_url

if TYPE_CHECKING:
    from collections.abc import Sequence

    from jinja2.environment import Environment
    from mkdocs.config.defaults import MkDocsConfig
    from mkdocs.structure.files import Files
    from mkdocs.structure.nav import Section
    from mkdocs.structure.toc import AnchorLink

try:
    from mkdocs.plugins import get_plugin_logger

    _log = get_plugin_logger(__name__)
except ImportError:
    # TODO: Remove once support for MkDocs <1.5 is dropped.
    _log = logging.getLogger(f"mkdocs.plugins.{__name__}")  # type: ignore[assignment]


class AutorefsConfig(Config):
    """Configuration options for the `autorefs` plugin."""

    resolve_closest: bool = Type(bool, default=False)  # type: ignore[assignment]
    """Whether to resolve an autoref to the closest URL when multiple URLs are found for an identifier.

    By closest, we mean a combination of "relative to the current page" and "shortest distance from the current page".

    For example, if you link to identifier `hello` from page `foo/bar/`,
    and the identifier is found in `foo/`, `foo/baz/` and `foo/bar/baz/qux/` pages,
    autorefs will resolve to `foo/bar/baz/qux`, which is the only URL relative to `foo/bar/`.

    If multiple URLs are equally close, autorefs will resolve to the first of these equally close URLs.
    If autorefs cannot find any URL that is close to the current page, it will log a warning and resolve to the first URL found.

    When false and multiple URLs are found for an identifier, autorefs will log a warning and resolve to the first URL.
    """

    link_titles: bool | Literal["auto", "external"] = Choice((True, False, "auto", "external"), default="auto")  # type: ignore[assignment]
    """Whether to set titles on links.

    Such title attributes are displayed as tooltips when hovering over the links.

    - `"auto"`: autorefs will detect whether the instant preview feature of Material for MkDocs is enabled,
        and set titles on external links when it is, all links if it is not.
    - `"external"`: autorefs will set titles on external links only.
    - `True`: autorefs will set titles on all links.
    - `False`: autorefs will not set any title attributes on links.

    Titles are only set when they are different from the link's text.
    Titles are constructed from the linked heading's original title,
    optionally appending the identifier for API objects.
    """

    strip_title_tags: bool | Literal["auto"] = Choice((True, False, "auto"), default="auto")  # type: ignore[assignment]
    """Whether to strip HTML tags from link titles.

    Some themes support HTML in link titles, but others do not.

    - `"auto"`: strip tags unless the Material for MkDocs theme is detected.
    """


class AutorefsPlugin(BasePlugin[AutorefsConfig]):
    """The `autorefs` plugin for `mkdocs`.

    This plugin defines the following event hooks:

    - `on_config`, to configure itself
    - `on_page_markdown`, to set the current page in order for Markdown extension to use it
    - `on_env`, to apply cross-references once all pages have been rendered

    Check the [Developing Plugins](https://www.mkdocs.org/user-guide/plugins/#developing-plugins) page of `mkdocs`
    for more information about its plugin system.
    """

    scan_toc: bool = True
    """Whether to scan the table of contents for identifiers to map to URLs."""
    record_backlinks: bool = False
    """Whether to record backlinks."""
    current_page: Page | None = None
    """The current page being processed."""
    # YORE: Bump 2: Remove block.
    legacy_refs: bool = True
    """Whether to support legacy references."""

    def __init__(self) -> None:
        """Initialize the object."""
        super().__init__()

        # The plugin uses three URL maps, one for "primary" URLs, one for "secondary" URLs,
        # and one for "absolute" URLs.
        #
        # - A primary URL is an identifier that links to a specific anchor on a page.
        # - A secondary URL is an alias of an identifier that links to the same anchor as the identifier's primary URL.
        #   Primary URLs with these aliases as identifiers may or may not be rendered later.
        # - An absolute URL is an identifier that links to an external resource.
        #   These URLs are typically registered by mkdocstrings when loading object inventories.
        #
        # For example, mkdocstrings registers a primary URL for each heading rendered in a page.
        # Then, for each alias of this heading's identifier, it registers a secondary URL.
        #
        # We need to keep track of whether an identifier is primary or secondary,
        # to give it precedence when resolving cross-references.
        # We wouldn't want to log a warning if there is a single primary URL and one or more secondary URLs,
        # instead we want to use the primary URL without any warning.
        #
        # - A single primary URL mapped to an identifer? Use it.
        # - Multiple primary URLs mapped to an identifier? Use the first one, or closest one if configured as such.
        # - No primary URL mapped to an identifier, but a secondary URL mapped? Use it.
        # - Multiple secondary URLs mapped to an identifier? Use the first one, or closest one if configured as such.
        # - No secondary URL mapped to an identifier? Try using absolute URLs
        #   (typically registered by loading inventories in mkdocstrings).
        #
        # This logic unfolds in `_get_item_url`.
        self._primary_url_map: dict[str, list[str]] = {}
        self._secondary_url_map: dict[str, list[str]] = {}
        self._title_map: dict[str, str] = {}
        self._breadcrumbs_map: dict[str, BacklinkCrumb] = {}
        self._abs_url_map: dict[str, str] = {}
        self._backlinks: dict[str, dict[str, set[str]]] = defaultdict(lambda: defaultdict(set))
        # YORE: Bump 2: Remove line.
        self._get_fallback_anchor: Callable[[str], tuple[str, ...]] | None = None
        # YORE: Bump 2: Remove line.
        self._url_to_page: dict[str, Page] = {}

        self._link_titles: bool | Literal["external"] = True
        self._strip_title_tags: bool = False

    # ----------------------------------------------------------------------- #
    # MkDocs Hooks                                                            #
    # ----------------------------------------------------------------------- #
    def on_config(self, config: MkDocsConfig) -> MkDocsConfig | None:
        """Instantiate our Markdown extension.

        Hook for the [`on_config` event](https://www.mkdocs.org/user-guide/plugins/#on_config).
        In this hook, we instantiate our [`AutorefsExtension`][mkdocs_autorefs.AutorefsExtension]
        and add it to the list of Markdown extensions used by `mkdocs`.

        Arguments:
            config: The MkDocs config object.

        Returns:
            The modified config.
        """
        _log.debug("Adding AutorefsExtension to the list")
        config.markdown_extensions.append(AutorefsExtension(self))  # type: ignore[arg-type]

        # YORE: Bump 2: Remove block.
        # mkdocstrings still uses the `page` attribute as a string.
        # Fortunately, it does so in f-strings, so we can simply patch the `__str__` method
        # to render the URL.
        Page.__str__ = lambda page: page.url  # type: ignore[method-assign,attr-defined]

        if self.config.link_titles == "auto":
            if getattr(config.theme, "name", None) == "material" and "navigation.instant.preview" in config.theme.get(
                "features",
                (),
            ):
                self._link_titles = "external"
            else:
                self._link_titles = True
        else:
            self._link_titles = self.config.link_titles

        if self.config.strip_title_tags == "auto":
            if getattr(config.theme, "name", None) == "material" and "content.tooltips" in config.theme.get(
                "features",
                (),
            ):
                self._strip_title_tags = False
            else:
                self._strip_title_tags = True
        else:
            self._strip_title_tags = self.config.strip_title_tags

        return config

    def on_page_markdown(self, markdown: str, page: Page, **kwargs: Any) -> str:  # noqa: ARG002
        """Remember which page is the current one.

        Arguments:
            markdown: Input Markdown.
            page: The related MkDocs page instance.
            kwargs: Additional arguments passed by MkDocs.

        Returns:
            The same Markdown. We only use this hook to keep a reference to the current page URL,
                used during Markdown conversion by the anchor/heading scanner tree processors.
        """
        # YORE: Bump 2: Remove line.
        self._url_to_page[page.url] = page
        self.current_page = page
        return markdown

    def on_page_content(self, html: str, page: Page, **kwargs: Any) -> str:  # noqa: ARG002
        """Register breadcrumbs.

        Hook for the [`on_page_content` event](https://www.mkdocs.org/user-guide/plugins/#on_page_content).
        In this hook, we register breadcrumbs for each heading on each page.
        These breadcrumbs are used to provide contextual information in backlinks.

        Arguments:
            html: HTML converted from Markdown.
            page: The related MkDocs page instance.
            kwargs: Additional arguments passed by MkDocs.

        Returns:
            The same HTML.
        """
        self.current_page = page
        if self.record_backlinks:
            for item in page.toc.items:
                self._register_breadcrumbs(page, item)
        return html

    @event_priority(-50)  # Late, after mkdocstrings has finished loading inventories.
    def on_env(self, env: Environment, /, *, config: MkDocsConfig, files: Files) -> Environment:  # noqa: ARG002
        """Apply cross-references and collect backlinks.

        Hook for the [`on_env` event](https://www.mkdocs.org/user-guide/plugins/#on_env).
        In this hook, we try to fix unresolved references of the form `[title][identifier]` or `[identifier][]`.
        Doing that allows the user of `autorefs` to cross-reference objects in their documentation strings.
        It uses the native Markdown syntax so it's easy to remember and use.

        We log a warning for each reference that we couldn't map to an URL.

        We also collect backlinks at the same time. We fix cross-refs and collect backlinks in a single pass
        for performance reasons (we don't want to run the regular expression on each page twice).

        Arguments:
            env: The MkDocs environment.
            config: The MkDocs config object.
            files: The list of files in the MkDocs project.

        Returns:
            The unmodified environment.
        """
        for file in files:
            if file.page and file.page.content:
                _log.debug("Applying cross-refs in page %s", file.page.file.src_path)

                url_mapper = functools.partial(
                    self.get_item_url,
                    from_url=file.page.url,
                    # YORE: Bump 2: Remove line.
                    fallback=self.get_fallback_anchor,
                )
                backlink_recorder = (
                    functools.partial(self._record_backlink, page_url=file.page.url) if self.record_backlinks else None
                )
                # YORE: Bump 2: Replace `, _legacy_refs=self.legacy_refs` with `` within line.
                file.page.content, unmapped = fix_refs(
                    file.page.content,
                    url_mapper,
                    record_backlink=backlink_recorder,
                    link_titles=self._link_titles,
                    strip_title_tags=self._strip_title_tags,
                    _legacy_refs=self.legacy_refs,
                )

                if unmapped and _log.isEnabledFor(logging.WARNING):
                    for ref, context in unmapped:
                        message = f"from {context.filepath}:{context.lineno}: ({context.origin}) " if context else ""
                        _log.warning(
                            f"{file.page.file.src_path}: {message}Could not find cross-reference target '{ref}'",
                        )

        return env

    # ----------------------------------------------------------------------- #
    # Utilities                                                               #
    # ----------------------------------------------------------------------- #
    # YORE: Bump 2: Remove block.
    def map_urls(self, page: Page, anchor: AnchorLink) -> None:
        """Deprecated. Recurse on every anchor to map its ID to its absolute URL.

        This method is deprecated and will be removed in a future release.
        Please use the `register_anchor` or `register_url` methods instead.

        This method populates `self._primary_url_map` by side-effect.

        Arguments:
            page: The page containing the anchors.
            anchor: The anchor to process and to recurse on.
        """
        warn(
            "The `map_urls` method is deprecated and will be removed in a future release. "
            "Please use the `register_anchor` or `register_url` methods instead.",
            DeprecationWarning,
            stacklevel=2,
        )
        self._map_urls(page, anchor)

    # YORE: Bump 2: Remove block.
    def _map_urls(self, page: Page | str, anchor: AnchorLink) -> None:
        if isinstance(page, str):
            try:
                page = self._url_to_page[page]
            except KeyError:
                page = self.current_page  # type: ignore[assignment]

        self.register_anchor(page, anchor.id, title=anchor.title, primary=True)  # type: ignore[arg-type]
        self._register_breadcrumbs(page, anchor)  # type: ignore[arg-type]

    def _register_breadcrumbs(self, page: Page, anchor: AnchorLink, parent: BacklinkCrumb | None = None) -> None:
        # Getting a breadcrumb has a side-effect of registering it in the breadcrumbs map.
        breadcrumb = self._get_breadcrumb(page, anchor, parent)
        for child in anchor.children:
            self._register_breadcrumbs(page, child, breadcrumb)

    def _get_breadcrumb(
        self,
        page: Page | Section,
        anchor: AnchorLink | None = None,
        parent: BacklinkCrumb | None = None,
    ) -> BacklinkCrumb:
        parent_breadcrumb = None if page.parent is None else self._get_breadcrumb(page.parent)
        if parent is None:
            if isinstance(page, Page):
                if (parent_url := page.url) not in self._breadcrumbs_map:
                    self._breadcrumbs_map[parent_url] = BacklinkCrumb(
                        title=page.title,
                        url=parent_url,
                        parent=parent_breadcrumb,
                    )
                parent = self._breadcrumbs_map[parent_url]
            else:
                parent = BacklinkCrumb(title=page.title, url="", parent=parent_breadcrumb)
        if anchor is None:
            return parent
        if (url := f"{page.url}#{anchor.id}") not in self._breadcrumbs_map:  # type: ignore[union-attr]
            # Skip the parent page if the anchor is a top-level heading, to reduce repetition.
            if anchor.level == 1:
                parent = parent.parent
            self._breadcrumbs_map[url] = BacklinkCrumb(title=anchor.title, url=url, parent=parent)
        return self._breadcrumbs_map[url]

    def _record_backlink(self, identifier: str, backlink_type: str, backlink_anchor: str, page_url: str) -> None:
        """Record a backlink.

        Arguments:
            identifier: The target identifier.
            backlink_type: The type of backlink.
            backlink_anchor: The backlink target anchor.
            page_url: The URL of the page containing the backlink.
        """
        # When we record backlinks, all identifiers have been registered.
        # If an identifier is not found in the primary or secondary URL maps, it's an absolute URL,
        # meaning it comes from an external source (typically an object inventory),
        # and we don't need to record backlinks for it.
        if identifier in self._primary_url_map or identifier in self._secondary_url_map:
            self._backlinks[identifier][backlink_type].add(f"{page_url}#{backlink_anchor}")

    def get_backlinks(self, *identifiers: str, from_url: str) -> dict[str, set[Backlink]]:
        """Return the backlinks to an identifier relative to the given URL.

        Arguments:
            *identifiers: The identifiers to get backlinks for.
            from_url: The URL of the page where backlinks are rendered.

        Returns:
            A dictionary of backlinks, with the type of reference as key and a set of backlinks as value.
            Each backlink is a tuple of (URL, title) tuples forming navigation breadcrumbs.
        """
        relative_backlinks: dict[str, set[Backlink]] = defaultdict(set)
        for identifier in set(identifiers):
            backlinks = self._backlinks.get(identifier, {})
            for backlink_type, backlink_urls in backlinks.items():
                for backlink_url in backlink_urls:
                    if backlink := self._get_backlink(from_url, backlink_url):
                        relative_backlinks[backlink_type].add(backlink)
        return relative_backlinks

    def _get_backlink(self, from_url: str, backlink_url: str) -> Backlink | None:
        breadcrumbs = []
        breadcrumb: BacklinkCrumb | None
        if not (breadcrumb := self._breadcrumbs_map.get(backlink_url)):
            _log.debug("No breadcrumb for backlink URL %s", backlink_url)
            return None
        while breadcrumb:
            breadcrumbs.append(
                BacklinkCrumb(
                    title=breadcrumb.title,
                    url=breadcrumb.url and relative_url(from_url, breadcrumb.url),
                    parent=breadcrumb.parent,
                ),
            )
            breadcrumb = breadcrumb.parent
        return Backlink(tuple(reversed(breadcrumbs)))

    def register_anchor(
        self,
        page: Page,
        identifier: str,
        anchor: str | None = None,
        *,
        title: str | None = None,
        primary: bool = True,
    ) -> None:
        """Register that an anchor corresponding to an identifier was encountered when rendering the page.

        Arguments:
            page: The page where the anchor was found.
            identifier: The identifier to register.
            anchor: The anchor on the page, without `#`. If not provided, defaults to the identifier.
            title: The title of the anchor (optional).
            primary: Whether this anchor is the primary one for the identifier.
        """
        # YORE: Bump 2: Remove block.
        if isinstance(page, str):
            try:
                page = self._url_to_page[page]
            except KeyError:
                page = self.current_page  # type: ignore[assignment]

        url = f"{page.url}#{anchor or identifier}"
        url_map = self._primary_url_map if primary else self._secondary_url_map
        if identifier in url_map:
            if url not in url_map[identifier]:
                url_map[identifier].append(url)
        else:
            url_map[identifier] = [url]
        if title and url not in self._title_map:
            self._title_map[url] = title

    def register_url(self, identifier: str, url: str) -> None:
        """Register that the identifier should be turned into a link to this URL.

        Arguments:
            identifier: The new identifier.
            url: The absolute URL (including anchor, if needed) where this item can be found.
        """
        self._abs_url_map[identifier] = url

    @staticmethod
    def _get_closest_url(from_url: str, urls: list[str], qualifier: str) -> str:
        """Return the closest URL to the current page.

        Arguments:
            from_url: The URL of the base page, from which we link towards the targeted pages.
            urls: A list of URLs to choose from.
            qualifier: The type of URLs we are choosing from.

        Returns:
            The closest URL to the current page.
        """
        base_url = URL(from_url)

        while True:
            if candidates := [url for url in urls if URL(url).is_relative_to(base_url)]:
                break
            base_url = base_url.parent
            if not base_url.name:
                break

        if not candidates:
            _log.warning(
                "Could not find closest %s URL (from %s, candidates: %s). "
                "Make sure to use unique headings, identifiers, or Markdown anchors (see our docs).",
                qualifier,
                from_url,
                urls,
            )
            return urls[0]

        winner = candidates[0] if len(candidates) == 1 else min(candidates, key=lambda c: c.count("/"))
        _log.debug("Closest URL found: %s (from %s, candidates: %s)", winner, from_url, urls)
        return winner

    def _get_urls(self, identifier: str) -> tuple[list[str], str]:
        try:
            return self._primary_url_map[identifier], "primary"
        except KeyError:
            return self._secondary_url_map[identifier], "secondary"

    def _get_item_url(
        self,
        identifier: str,
        from_url: str | None = None,
        # YORE: Bump 2: Remove line.
        fallback: Callable[[str], Sequence[str]] | None = None,
    ) -> str:
        try:
            urls, qualifier = self._get_urls(identifier)
        except KeyError:
            # YORE: Bump 2: Replace block with line 2.
            if identifier in self._abs_url_map:
                return self._abs_url_map[identifier]
            if fallback:
                new_identifiers = fallback(identifier)
                for new_identifier in new_identifiers:
                    with contextlib.suppress(KeyError):
                        url = self._get_item_url(new_identifier)
                        self._secondary_url_map[identifier] = [url]
                        return url
            raise

        if len(urls) > 1:
            if (self.config.resolve_closest or qualifier == "secondary") and from_url is not None:
                return self._get_closest_url(from_url, urls, qualifier)
            _log.warning(
                "Multiple %s URLs found for '%s': %s. "
                "Make sure to use unique headings, identifiers, or Markdown anchors (see our docs).",
                qualifier,
                identifier,
                urls,
            )
        return urls[0]

    def get_item_url(
        self,
        identifier: str,
        from_url: str | None = None,
        # YORE: Bump 2: Remove line.
        fallback: Callable[[str], Sequence[str]] | None = None,
    ) -> tuple[str, str | None]:
        """Return a site-relative URL with anchor to the identifier, if it's present anywhere.

        Arguments:
            identifier: The anchor (without '#').
            from_url: The URL of the base page, from which we link towards the targeted pages.

        Returns:
            A site-relative URL.
        """
        # YORE: Bump 2: Replace `, fallback` with `` within line.
        url = self._get_item_url(identifier, from_url, fallback)
        title = self._title_map.get(url) or None
        if from_url is not None:
            parsed = urlsplit(url)
            if not parsed.scheme and not parsed.netloc:
                url = relative_url(from_url, url)
        return url, title

    # YORE: Bump 2: Remove block.
    # ----------------------------------------------------------------------- #
    # Deprecated API                                                          #
    # ----------------------------------------------------------------------- #
    @property
    def get_fallback_anchor(self) -> Callable[[str], tuple[str, ...]] | None:
        """Fallback anchors getter."""
        return self._get_fallback_anchor

    # YORE: Bump 2: Remove block.
    @get_fallback_anchor.setter
    def get_fallback_anchor(self, value: Callable[[str], tuple[str, ...]] | None) -> None:
        """Fallback anchors setter."""
        self._get_fallback_anchor = value
        if value is not None:
            warn(
                "Setting a fallback anchor function is deprecated and will be removed in a future release.",
                DeprecationWarning,
                stacklevel=2,
            )
