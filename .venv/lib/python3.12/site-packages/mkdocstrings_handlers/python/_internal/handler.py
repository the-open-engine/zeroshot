# This module implements a handler for the Python language.

from __future__ import annotations

import glob
import inspect
import os
import posixpath
import sys
from contextlib import suppress
from dataclasses import asdict
from pathlib import Path
from typing import TYPE_CHECKING, Any, BinaryIO, Callable, ClassVar

from griffe import (
    AliasResolutionError,
    GriffeLoader,
    LinesCollection,
    ModulesCollection,
    Parser,
    load_extensions,
    parse_auto,
    parse_google,
    parse_numpy,
    parse_sphinx,
    patch_loggers,
)
from mkdocs.exceptions import PluginError
from mkdocs_autorefs import BacklinkCrumb
from mkdocstrings import BaseHandler, CollectionError, CollectorItem, HandlerOptions, Inventory, get_logger

from mkdocstrings_handlers.python._internal import rendering
from mkdocstrings_handlers.python._internal.config import PythonConfig, PythonOptions

if TYPE_CHECKING:
    from collections.abc import Iterable, Iterator, Mapping, MutableMapping, Sequence

    from mkdocs.config.defaults import MkDocsConfig
    from mkdocs_autorefs import Backlink


# YORE: EOL 3.10: Replace block with line 2.
if sys.version_info >= (3, 11):
    from contextlib import chdir
else:
    from contextlib import contextmanager

    @contextmanager
    def chdir(path: str) -> Iterator[None]:
        old_wd = os.getcwd()
        os.chdir(path)
        try:
            yield
        finally:
            os.chdir(old_wd)


_logger = get_logger(__name__)

patch_loggers(get_logger)

_PARSER_FUNCTIONS: dict[Parser, Callable] = {
    Parser.auto: parse_auto,
    Parser.google: parse_google,
    Parser.numpy: parse_numpy,
    Parser.sphinx: parse_sphinx,
}


def _filter_parser_options(parser: Parser | None, options: dict[str, Any] | None) -> dict[str, Any] | None:
    """Filter options unsupported by the selected Griffe parser."""
    if parser is None or options is None:
        return options

    accepted_options = set(inspect.signature(_PARSER_FUNCTIONS[parser]).parameters) - {"docstring"}
    filtered_options = {}
    for name, value in options.items():
        if name in accepted_options:
            if parser is Parser.auto and name == "per_style_options":
                filtered_options[name] = {
                    style: _filter_parser_options(Parser(style), style_options)
                    for style, style_options in value.items()
                }
            else:
                filtered_options[name] = value
        else:
            _logger.warning(f"Ignoring unsupported {parser.value} docstring parser option: {name}")
    return filtered_options


class PythonHandler(BaseHandler):
    """The Python handler class."""

    name: ClassVar[str] = "python"
    """The handler's name."""

    domain: ClassVar[str] = "py"
    """The cross-documentation domain/language for this handler."""

    enable_inventory: ClassVar[bool] = True
    """Whether this handler is interested in enabling the creation of the `objects.inv` Sphinx inventory file."""

    fallback_theme: ClassVar[str] = "material"
    """The fallback theme."""

    def __init__(self, config: PythonConfig, base_dir: Path, **kwargs: Any) -> None:
        """Initialize the handler.

        Parameters:
            config: The handler configuration.
            base_dir: The base directory of the project.
            **kwargs: Arguments passed to the parent constructor.
        """
        super().__init__(**kwargs)

        self.config = config
        """The handler configuration."""
        self.base_dir = base_dir
        """The base directory of the project."""

        self.global_options = config.options
        """The global configuration options (in `mkdocs.yml`)."""

        # Warn if user overrides base templates.
        if self.custom_templates:
            for theme_dir in base_dir.joinpath(self.custom_templates, "python").iterdir():
                if theme_dir.joinpath("_base").is_dir():
                    _logger.warning(
                        f"Overriding base template '{theme_dir.name}/_base/<template>.html.jinja' is not supported, "
                        f"override '{theme_dir.name}/<template>.html.jinja' instead",
                    )

        paths = config.paths or []

        # Expand paths with glob patterns.
        with chdir(str(base_dir)):
            resolved_globs = [glob.glob(path) for path in paths]
        paths = [path for glob_list in resolved_globs for path in glob_list]

        # By default, add the base directory to the search paths.
        if not paths:
            paths.append(str(base_dir))

        # Initialize search paths from `sys.path`, eliminating empty paths.
        search_paths = [path for path in sys.path if path]

        for path in reversed(paths):
            # If it's not absolute, make path relative to the config file path, then make it absolute.
            if not os.path.isabs(path):
                path = os.path.abspath(base_dir / path)  # noqa: PLW2901
            # Remove pre-listed paths.
            if path in search_paths:
                search_paths.remove(path)
            # Give precedence to user-provided paths.
            search_paths.insert(0, path)

        self._paths = search_paths
        self._modules_collection: ModulesCollection = ModulesCollection()
        self._lines_collection: LinesCollection = LinesCollection()

    def get_inventory_urls(self) -> list[tuple[str, dict[str, Any]]]:
        """Return the URLs of the inventory files to download."""
        return [(inv.url, inv._config) for inv in self.config.inventories]

    @staticmethod
    def load_inventory(
        in_file: BinaryIO,
        url: str,
        base_url: str | None = None,
        domains: list[str] | None = None,
        **kwargs: Any,  # noqa: ARG004
    ) -> Iterator[tuple[str, str]]:
        """Yield items and their URLs from an inventory file streamed from `in_file`.

        This implements mkdocstrings' `load_inventory` "protocol" (see [`mkdocstrings.BaseHandler.load_inventory`][]).

        Arguments:
            in_file: The binary file-like object to read the inventory from.
            url: The URL that this file is being streamed from (used to guess `base_url`).
            base_url: The URL that this inventory's sub-paths are relative to.
            domains: A list of domain strings to filter the inventory by, when not passed, "py" will be used.
            **kwargs: Ignore additional arguments passed from the config.

        Yields:
            Tuples of (item identifier, item URL).
        """
        domains = domains or ["py"]
        if base_url is None:
            base_url = posixpath.dirname(url)

        for item in Inventory.parse_sphinx(in_file, domain_filter=domains).values():
            yield item.name, posixpath.join(base_url, item.uri)

    def get_options(self, local_options: Mapping[str, Any]) -> HandlerOptions:
        """Get combined default, global and local options.

        Arguments:
            local_options: The local options.

        Returns:
            The combined options.
        """
        extra = {**self.global_options.get("extra", {}), **local_options.get("extra", {})}
        options = {**self.global_options, **local_options, "extra": extra}
        try:
            return PythonOptions.from_data(**options)
        except Exception as error:
            raise PluginError(f"Invalid options: {error}") from error

    def collect(self, identifier: str, options: PythonOptions) -> CollectorItem:
        """Collect the documentation for the given identifier.

        Parameters:
            identifier: The identifier of the object to collect.
            options: The options to use for the collection.

        Returns:
            The collected item.
        """
        module_name = identifier.split(".", 1)[0]
        unknown_module = module_name not in self._modules_collection
        reapply = True
        if options == {}:
            if unknown_module:
                raise CollectionError("Not loading additional modules during fallback")
            options = self.get_options({})
            reapply = False

        parser_name = options.docstring_style
        parser = parser_name and Parser(parser_name)
        parser_options: dict[str, Any] | None = None
        if options.docstring_options is not None:
            parser_options = _filter_parser_options(parser, asdict(options.docstring_options))

        if unknown_module:
            extensions = self.normalize_extension_paths(options.extensions)
            loader = GriffeLoader(
                extensions=load_extensions(*extensions),
                search_paths=self._paths,
                docstring_parser=parser,
                docstring_options=parser_options,  # type: ignore[arg-type]
                modules_collection=self._modules_collection,
                lines_collection=self._lines_collection,
                allow_inspection=options.allow_inspection,
                force_inspection=options.force_inspection,
            )
            try:
                for pre_loaded_module in options.preload_modules:
                    if pre_loaded_module not in self._modules_collection:
                        loader.load(
                            pre_loaded_module,
                            try_relative_path=False,
                            find_stubs_package=options.find_stubs_package,
                        )
                loader.load(
                    module_name,
                    try_relative_path=False,
                    find_stubs_package=options.find_stubs_package,
                )
            except ImportError as error:
                raise CollectionError(str(error)) from error
            unresolved, iterations = loader.resolve_aliases(
                implicit=False,
                external=self.config.load_external_modules,
            )
            if unresolved:
                _logger.debug(f"{len(unresolved)} aliases were still unresolved after {iterations} iterations")
                _logger.debug(f"Unresolved aliases: {', '.join(sorted(unresolved))}")

        try:
            doc_object = self._modules_collection[identifier]
        except KeyError as error:
            raise CollectionError(f"{identifier} could not be found") from error
        except AliasResolutionError as error:
            raise CollectionError(str(error)) from error

        if not unknown_module and reapply:
            with suppress(AliasResolutionError):
                if doc_object.docstring is not None:
                    doc_object.docstring.parser = parser
                    doc_object.docstring.parser_options = parser_options or {}

        return doc_object

    def render(self, data: CollectorItem, options: PythonOptions, locale: str | None = None) -> str:
        """Render the collected data.

        Parameters:
            data: The collected data.
            options: The options to use for rendering.
            locale: The locale to use for rendering (default is "en").

        Returns:
            The rendered data (HTML).
        """
        template_name = rendering.do_get_template(data)
        template = self.env.get_template(template_name)

        return template.render(
            **{
                "config": options,
                data.kind.value.replace(" ", "_"): data,
                # Heading level is a "state" variable, that will change at each step
                # of the rendering recursion. Therefore, it's easier to use it as a plain value
                # than as an item in a dictionary.
                "heading_level": options.heading_level,
                "root": True,
                "locale": locale or "en",
            },
        )

    def render_backlinks(self, backlinks: Mapping[str, Iterable[Backlink]], *, locale: str | None = None) -> str:  # noqa: ARG002
        """Render the backlinks.

        Parameters:
            backlinks: The backlinks to render.

        Returns:
            The rendered backlinks (HTML).
        """
        template = self.env.get_template("backlinks.html.jinja")
        verbose_type = {key: key.capitalize().replace("-by", " by") for key in backlinks.keys()}  # noqa: SIM118
        return template.render(
            backlinks=backlinks,
            config=self.get_options({}),
            verbose_type=verbose_type,
            default_crumb=BacklinkCrumb(title="", url=""),
        )

    def update_env(self, config: Any) -> None:  # noqa: ARG002
        """Update the Jinja environment with custom filters and tests.

        Parameters:
            config: The SSG configuration.
        """
        self.env.trim_blocks = True
        self.env.lstrip_blocks = True
        self.env.keep_trailing_newline = False
        self.env.filters["split_path"] = rendering.do_split_path
        self.env.filters["order_members"] = rendering.do_order_members
        self.env.filters["format_code"] = rendering.do_format_code
        self.env.filters["format_signature"] = rendering.do_format_signature
        self.env.filters["format_attribute"] = rendering.do_format_attribute
        self.env.filters["format_type_alias"] = rendering.do_format_type_alias
        self.env.filters["filter_objects"] = rendering.do_filter_objects
        self.env.filters["stash_crossref"] = rendering.do_stash_crossref
        self.env.filters["get_template"] = rendering.do_get_template
        self.env.filters["source_location"] = rendering.do_source_location
        self.env.filters["as_attributes_section"] = rendering.do_as_attributes_section
        self.env.filters["as_functions_section"] = rendering.do_as_functions_section
        self.env.filters["as_classes_section"] = rendering.do_as_classes_section
        self.env.filters["as_type_aliases_section"] = rendering.do_as_type_aliases_section
        self.env.filters["as_modules_section"] = rendering.do_as_modules_section
        self.env.filters["backlink_tree"] = rendering.do_backlink_tree
        self.env.globals["AutorefsHook"] = rendering.AutorefsHook
        self.env.tests["existing_template"] = rendering._test_existing_template  # type: ignore[assignment]

    def get_aliases(self, identifier: str) -> tuple[str, ...]:
        """Get the aliases for the given identifier.

        Parameters:
            identifier: The identifier to get the aliases for.

        Returns:
            The aliases.
        """
        if "(" in identifier:
            identifier, parameter = identifier.split("(", 1)
            parameter = parameter.removesuffix(")")
        else:
            parameter = ""
        try:
            data = self._modules_collection[identifier]
        except (KeyError, AliasResolutionError):
            return ()
        aliases = [data.path]
        try:
            for alias in [data.canonical_path, *data.aliases]:
                if alias not in aliases:
                    aliases.append(alias)
        except AliasResolutionError:
            pass
        if parameter:
            return tuple(f"{alias}({parameter})" for alias in aliases)
        return tuple(aliases)

    def normalize_extension_paths(self, extensions: Sequence) -> list[str | dict[str, Any]]:
        """Resolve extension paths relative to config file.

        Parameters:
            extensions: The extensions (configuration) to normalize.

        Returns:
            The normalized extensions.
        """
        normalized: list[str | dict[str, Any]] = []

        for ext in extensions:
            if isinstance(ext, dict):
                pth, options = next(iter(ext.items()))
                pth = str(pth)
            else:
                pth = str(ext)
                options = None

            if pth.endswith(".py") or ".py:" in pth or "/" in pth or "\\" in pth:
                # This is a system path. Normalize it, make it absolute relative to config file path.
                pth = os.path.abspath(self.base_dir / pth)

            if options is not None:
                normalized.append({pth: options})
            else:
                normalized.append(pth)

        return normalized


def get_handler(
    handler_config: MutableMapping[str, Any],
    tool_config: MkDocsConfig,
    **kwargs: Any,
) -> PythonHandler:
    """Return an instance of `PythonHandler`.

    Parameters:
        handler_config: The handler configuration.
        tool_config: The tool (SSG) configuration.
        **kwargs: Additional arguments to pass to the handler.

    Returns:
        An instance of `PythonHandler`.
    """
    # In rare cases, Griffe hits the recursion limit because of deeply-nested ASTs.
    # We therefore increase the limit here, once, before Griffe is used to collect or render stuff.
    sys.setrecursionlimit(max(sys.getrecursionlimit(), 2000))

    base_dir = Path(getattr(tool_config, "config_file_path", None) or "./mkdocs.yml").parent
    return PythonHandler(
        config=PythonConfig.from_data(**handler_config),
        base_dir=base_dir,
        **kwargs,
    )
