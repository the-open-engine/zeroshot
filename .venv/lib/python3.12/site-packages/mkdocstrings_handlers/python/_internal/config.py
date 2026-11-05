# Configuration and options dataclasses.

from __future__ import annotations

import re
import sys
from dataclasses import field, fields
from typing import TYPE_CHECKING, Annotated, Any, Literal

from mkdocstrings import get_logger

from mkdocstrings_handlers.python._internal.rendering import Order  # noqa: TC001

# YORE: EOL 3.10: Replace block with line 2.
if sys.version_info >= (3, 11):
    from typing import Self
else:
    from typing_extensions import Self


_logger = get_logger(__name__)

_DEFAULT_FILTERS = ["!^_[^_]"]

try:
    # When Pydantic is available, use it to validate options (done automatically).
    # Users can therefore opt into validation by installing Pydantic in development/CI.
    # When building the docs to deploy them, Pydantic is not required anymore.

    # When building our own docs, Pydantic is always installed (see `docs` group in `pyproject.toml`)
    # to allow automatic generation of a JSON Schema. The JSON Schema is then referenced by mkdocstrings,
    # which is itself referenced by mkdocs-material's schema system. For example in VSCode:
    #
    # "yaml.schemas": {
    #     "https://squidfunk.github.io/mkdocs-material/schema.json": "mkdocs.yml"
    # }
    import pydantic

    if getattr(pydantic, "__version__", "1.").startswith("1."):
        raise ImportError  # noqa: TRY301

    from inspect import cleandoc

    from pydantic import Field as BaseField
    from pydantic.dataclasses import dataclass

    _base_url = "https://mkdocstrings.github.io/python/usage"

    def _Field(  # noqa: N802
        *args: Any,
        description: str,
        group: Literal["general", "headings", "members", "docstrings", "signatures"] | None = None,
        parent: str | None = None,
        **kwargs: Any,
    ) -> None:
        def _add_markdown_description(schema: dict[str, Any]) -> None:
            url = f"{_base_url}/{f'configuration/{group}/' if group else ''}#{parent or schema['title']}"
            schema["markdownDescription"] = f"[DOCUMENTATION]({url})\n\n{schema['description']}"

        return BaseField(
            *args,
            description=cleandoc(description),
            field_title_generator=lambda name, _: name,
            json_schema_extra=_add_markdown_description,
            **kwargs,
        )
except ImportError:
    from dataclasses import dataclass  # type: ignore[no-redef]

    def _Field(*args: Any, **kwargs: Any) -> None:  # type: ignore[misc]  # noqa: N802
        pass


if TYPE_CHECKING:
    from collections.abc import MutableMapping


@dataclass(frozen=True, kw_only=True)
class GoogleStyleOptions:
    """Google style docstring options."""

    ignore_init_summary: Annotated[
        bool,
        _Field(
            group="docstrings",
            parent="docstring_options",
            description="Whether to ignore the summary in `__init__` methods' docstrings.",
        ),
    ] = False

    returns_multiple_items: Annotated[
        bool,
        _Field(
            group="docstrings",
            parent="docstring_options",
            description="""Whether to parse multiple items in `Yields` and `Returns` sections.

            When true, each item's continuation lines must be indented.
            When false (single item), no further indentation is required.
            """,
        ),
    ] = True

    returns_named_value: Annotated[
        bool,
        _Field(
            group="docstrings",
            parent="docstring_options",
            description="""Whether to parse `Yields` and `Returns` section items as name and description, rather than type and description.

            When true, type must be wrapped in parentheses: `(int): Description.`. Names are optional: `name (int): Description.`.
            When false, parentheses are optional but the items cannot be named: `int: Description`.
            """,
        ),
    ] = True

    returns_type_in_property_summary: Annotated[
        bool,
        _Field(
            group="docstrings",
            parent="docstring_options",
            description="Whether to parse the return type of properties at the beginning of their summary: `str: Summary of the property`.",
        ),
    ] = False

    receives_multiple_items: Annotated[
        bool,
        _Field(
            group="docstrings",
            parent="docstring_options",
            description="""Whether to parse multiple items in `Receives` sections.

            When true, each item's continuation lines must be indented.
            When false (single item), no further indentation is required.
            """,
        ),
    ] = True

    receives_named_value: Annotated[
        bool,
        _Field(
            group="docstrings",
            parent="docstring_options",
            description="""Whether to parse `Receives` section items as name and description, rather than type and description.

            When true, type must be wrapped in parentheses: `(int): Description.`. Names are optional: `name (int): Description.`.
            When false, parentheses are optional but the items cannot be named: `int: Description`.
            """,
        ),
    ] = True

    trim_doctest_flags: Annotated[
        bool,
        _Field(
            group="docstrings",
            parent="docstring_options",
            description="Whether to remove doctest flags from Python example blocks.",
        ),
    ] = True

    warn_unknown_params: Annotated[
        bool,
        _Field(
            group="docstrings",
            parent="docstring_options",
            description="Warn about documented parameters not appearing in the signature.",
        ),
    ] = True

    warn_missing_types: Annotated[
        bool,
        _Field(
            group="docstrings",
            parent="docstring_options",
            description="Warn about missing type/annotation for parameters, return values, etc.",
        ),
    ] = True

    warnings: Annotated[
        bool,
        _Field(
            group="docstrings",
            parent="docstring_options",
            description="Generally enable/disable warnings when parsing docstrings.",
        ),
    ] = True


@dataclass(frozen=True, kw_only=True)
class NumpyStyleOptions:
    """Numpy style docstring options."""

    ignore_init_summary: Annotated[
        bool,
        _Field(
            group="docstrings",
            parent="docstring_options",
            description="Whether to ignore the summary in `__init__` methods' docstrings.",
        ),
    ] = False

    trim_doctest_flags: Annotated[
        bool,
        _Field(
            group="docstrings",
            parent="docstring_options",
            description="Whether to remove doctest flags from Python example blocks.",
        ),
    ] = True

    warn_unknown_params: Annotated[
        bool,
        _Field(
            group="docstrings",
            parent="docstring_options",
            description="Warn about documented parameters not appearing in the signature.",
        ),
    ] = True

    warn_missing_types: Annotated[
        bool,
        _Field(
            group="docstrings",
            parent="docstring_options",
            description="Warn about missing type/annotation for parameters, return values, etc.",
        ),
    ] = True

    warnings: Annotated[
        bool,
        _Field(
            group="docstrings",
            parent="docstring_options",
            description="Generally enable/disable warnings when parsing docstrings.",
        ),
    ] = True


@dataclass(frozen=True, kw_only=True)
class SphinxStyleOptions:
    """Sphinx style docstring options."""

    warn_unknown_params: Annotated[
        bool,
        _Field(
            group="docstrings",
            parent="docstring_options",
            description="Warn about documented parameters not appearing in the signature.",
        ),
    ] = True

    warn_missing_types: Annotated[
        bool,
        _Field(
            group="docstrings",
            parent="docstring_options",
            description="Warn about missing type/annotation for return values.",
        ),
    ] = True

    warnings: Annotated[
        bool,
        _Field(
            group="docstrings",
            parent="docstring_options",
            description="Generally enable/disable warnings when parsing docstrings.",
        ),
    ] = True


@dataclass(frozen=True, kw_only=True)
class PerStyleOptions:
    """Per style options."""

    google: Annotated[
        GoogleStyleOptions,
        _Field(
            group="docstrings",
            parent="docstring_options",
            description="Google-style options.",
        ),
    ] = field(default_factory=GoogleStyleOptions)

    numpy: Annotated[
        NumpyStyleOptions,
        _Field(
            group="docstrings",
            parent="docstring_options",
            description="Numpydoc-style options.",
        ),
    ] = field(default_factory=NumpyStyleOptions)

    sphinx: Annotated[
        SphinxStyleOptions,
        _Field(
            group="docstrings",
            parent="docstring_options",
            description="Sphinx-style options.",
        ),
    ] = field(default_factory=SphinxStyleOptions)

    @classmethod
    def from_data(cls, **data: Any) -> Self:
        """Create an instance from a dictionary."""
        if "google" in data:
            data["google"] = GoogleStyleOptions(**data["google"])
        if "numpy" in data:
            data["numpy"] = NumpyStyleOptions(**data["numpy"])
        if "sphinx" in data:
            data["sphinx"] = SphinxStyleOptions(**data["sphinx"])
        return cls(**data)


@dataclass(frozen=True, kw_only=True)
class AutoStyleOptions:
    """Auto style docstring options."""

    method: Annotated[
        Literal["heuristics", "max_sections"],
        _Field(
            group="docstrings",
            parent="docstring_options",
            description="The method to use to determine the docstring style.",
        ),
    ] = "heuristics"

    style_order: Annotated[
        list[str],
        _Field(
            group="docstrings",
            parent="docstring_options",
            description="The order of the docstring styles to try.",
        ),
    ] = field(default_factory=lambda: ["sphinx", "google", "numpy"])

    default: Annotated[
        str | None,
        _Field(
            group="docstrings",
            parent="docstring_options",
            description="The default docstring style to use if no other style is detected.",
        ),
    ] = None

    per_style_options: Annotated[
        PerStyleOptions,
        _Field(
            group="docstrings",
            parent="docstring_options",
            description="Per-style options.",
        ),
    ] = field(default_factory=PerStyleOptions)

    @classmethod
    def from_data(cls, **data: Any) -> Self:
        """Create an instance from a dictionary."""
        if "per_style_options" in data:
            data["per_style_options"] = PerStyleOptions.from_data(**data["per_style_options"])
        return cls(**data)


@dataclass(frozen=True, kw_only=True)
class SummaryOption:
    """Summary option."""

    attributes: Annotated[
        bool,
        _Field(
            group="members",
            parent="summary",
            description="Whether to render summaries of attributes.",
        ),
    ] = False

    functions: Annotated[
        bool,
        _Field(
            group="members",
            parent="summary",
            description="Whether to render summaries of functions (methods).",
        ),
    ] = False

    classes: Annotated[
        bool,
        _Field(
            group="members",
            parent="summary",
            description="Whether to render summaries of classes.",
        ),
    ] = False

    modules: Annotated[
        bool,
        _Field(
            group="members",
            parent="summary",
            description="Whether to render summaries of modules.",
        ),
    ] = False

    type_aliases: Annotated[
        bool,
        _Field(
            group="members",
            parent="summary",
            description="Whether to render summaries of type aliases.",
        ),
    ] = False


@dataclass(frozen=True, kw_only=True)
class PythonInputOptions:
    """Accepted input options."""

    allow_inspection: Annotated[
        bool,
        _Field(
            group="general",
            description="Whether to allow inspecting modules when visiting them is not possible.",
        ),
    ] = True

    force_inspection: Annotated[
        bool,
        _Field(
            group="general",
            description="Whether to force using dynamic analysis when loading data.",
        ),
    ] = False

    annotations_path: Annotated[
        Literal["brief", "source", "full"],
        _Field(
            group="signatures",
            description="The verbosity for annotations path: `brief` (recommended), `source` (as written in the source), or `full`.",
        ),
    ] = "brief"

    backlinks: Annotated[
        Literal["flat", "tree", False],
        _Field(
            group="general",
            description="Whether to render backlinks, and how.",
        ),
    ] = False

    docstring_options: Annotated[
        GoogleStyleOptions | NumpyStyleOptions | SphinxStyleOptions | AutoStyleOptions | None,
        _Field(
            group="docstrings",
            description="""The options for the docstring parser.

            See [docstring parsers](https://mkdocstrings.github.io/griffe/reference/docstrings/) and their options in Griffe docs.
            """,
        ),
    ] = None

    docstring_section_style: Annotated[
        Literal["table", "list", "spacy"],
        _Field(
            group="docstrings",
            description="The style used to render docstring sections.",
        ),
    ] = "table"

    docstring_style: Annotated[
        Literal["auto", "google", "numpy", "sphinx"] | None,
        _Field(
            group="docstrings",
            description="The docstring style to use: `auto`, `google`, `numpy`, `sphinx`, or `None`.",
        ),
    ] = "google"

    extensions: Annotated[
        list[str | dict[str, Any]],
        _Field(
            group="general",
            description="A list of Griffe extensions to load.",
        ),
    ] = field(default_factory=list)

    filters: Annotated[
        list[str] | Literal["public"],
        _Field(
            group="members",
            description="""A list of filters, or `"public"`.

            **List of filters**

            A filter starting with `!` will exclude matching objects instead of including them.
            The `members` option takes precedence over `filters` (filters will still be applied recursively
            to lower members in the hierarchy).

            **Filtering methods**

            The `public` method will include only public objects:
            those added to `__all__` or not starting with an underscore (except for special methods/attributes).
            """,
        ),
    ] = field(default_factory=_DEFAULT_FILTERS.copy)

    find_stubs_package: Annotated[
        bool,
        _Field(
            group="general",
            description="Whether to load stubs package (package-stubs) when extracting docstrings.",
        ),
    ] = False

    group_by_category: Annotated[
        bool,
        _Field(
            group="members",
            description="Group the object's children by categories: attributes, classes, functions, and modules.",
        ),
    ] = True

    heading: Annotated[
        str,
        _Field(
            group="headings",
            description="A custom string to override the autogenerated heading of the root object.",
        ),
    ] = ""

    heading_level: Annotated[
        int,
        _Field(
            group="headings",
            description="The initial heading level to use.",
        ),
    ] = 2

    inheritance_diagram_direction: Annotated[
        Literal["TB", "TD", "BT", "RL", "LR"],
        _Field(
            group="docstrings",
            description="The direction of the Mermaid chart presenting the inheritance diagram of a class.",
        ),
    ] = "TD"

    inherited_members: Annotated[
        bool | list[str],
        _Field(
            group="members",
            description="""A boolean, or an explicit list of inherited members to render.

            If true, select all inherited members, which can then be filtered with `members`.
            If false or empty list, do not select any inherited member.
            """,
        ),
    ] = False

    line_length: Annotated[
        int,
        _Field(
            group="signatures",
            description="Maximum line length when formatting code/signatures.",
        ),
    ] = 60

    members: Annotated[
        list[str] | bool | None,
        _Field(
            group="members",
            description="""A boolean, or an explicit list of members to render.

            If true, select all members without further filtering.
            If false or empty list, do not render members.
            If none, select all members and apply further filtering with filters and docstrings.
            """,
        ),
    ] = None

    members_order: Annotated[
        Order | list[Order],
        _Field(
            group="members",
            description="""The members ordering to use.

            - `__all__`: order members according to `__all__` module attributes, if declared;
            - `alphabetical`: order members alphabetically;
            - `source`: order members as they appear in the source file.

            Since `__all__` is a module-only attribute, it can't be used to sort class members,
            therefore the `members_order` option accepts a list of ordering methods,
            indicating ordering preferences.
            """,
        ),
    ] = "alphabetical"

    merge_init_into_class: Annotated[
        bool,
        _Field(
            group="docstrings",
            description="Whether to merge the `__init__` method into the class' signature and docstring.",
        ),
    ] = False

    modernize_annotations: Annotated[
        bool,
        _Field(
            group="signatures",
            description="Whether to modernize annotations, for example `Optional[str]` into `str | None`.",
        ),
    ] = False

    overloads_only: Annotated[
        bool,
        _Field(
            group="signatures",
            description="Whether to hide the implementation signature if the overloads are shown.",
        ),
    ] = False

    parameter_headings: Annotated[
        bool,
        _Field(
            group="headings",
            description="Whether to render headings for parameters (therefore showing parameters in the ToC).",
        ),
    ] = False

    preload_modules: Annotated[
        list[str],
        _Field(
            group="general",
            description="""Pre-load modules that are not specified directly in autodoc instructions (`::: identifier`).

            It is useful when you want to render documentation for a particular member of an object,
            and this member is imported from another package than its parent.

            For an imported member to be rendered, you need to add it to the `__all__` attribute
            of the importing module.

            The modules must be listed as an array of strings.
            """,
        ),
    ] = field(default_factory=list)

    relative_crossrefs: Annotated[
        bool,
        _Field(
            group="docstrings",
            description="Whether to enable the relative crossref syntax.",
        ),
    ] = False

    scoped_crossrefs: Annotated[
        bool,
        _Field(
            group="docstrings",
            description="Whether to enable the scoped crossref ability.",
        ),
    ] = False

    show_overloads: Annotated[
        bool,
        _Field(
            group="signatures",
            description="Show the overloads of a function or method.",
        ),
    ] = True

    separate_signature: Annotated[
        bool,
        _Field(
            group="signatures",
            description="""Whether to put the whole signature in a code block below the heading.

            If Black or Ruff are installed, the signature is also formatted using them.
            """,
        ),
    ] = False

    show_attribute_values: Annotated[
        bool,
        _Field(
            group="signatures",
            description="Show initial values of attributes in classes.",
        ),
    ] = True

    show_bases: Annotated[
        bool,
        _Field(
            group="general",
            description="Show the base classes of a class.",
        ),
    ] = True

    show_category_heading: Annotated[
        bool,
        _Field(
            group="headings",
            description="When grouped by categories, show a heading for each category.",
        ),
    ] = False

    show_docstring_attributes: Annotated[
        bool,
        _Field(
            group="docstrings",
            description="Whether to display the 'Attributes' section in the object's docstring.",
        ),
    ] = True

    show_docstring_classes: Annotated[
        bool,
        _Field(
            group="docstrings",
            description="Whether to display the 'Classes' section in the object's docstring.",
        ),
    ] = True

    show_docstring_description: Annotated[
        bool,
        _Field(
            group="docstrings",
            description="Whether to display the textual block (including admonitions) in the object's docstring.",
        ),
    ] = True

    show_docstring_examples: Annotated[
        bool,
        _Field(
            group="docstrings",
            description="Whether to display the 'Examples' section in the object's docstring.",
        ),
    ] = True

    show_docstring_functions: Annotated[
        bool,
        _Field(
            group="docstrings",
            description="Whether to display the 'Functions' or 'Methods' sections in the object's docstring.",
        ),
    ] = True

    show_docstring_modules: Annotated[
        bool,
        _Field(
            group="docstrings",
            description="Whether to display the 'Modules' section in the object's docstring.",
        ),
    ] = True

    show_docstring_other_parameters: Annotated[
        bool,
        _Field(
            group="docstrings",
            description="Whether to display the 'Other Parameters' section in the object's docstring.",
        ),
    ] = True

    show_docstring_parameters: Annotated[
        bool,
        _Field(
            group="docstrings",
            description="Whether to display the 'Parameters' section in the object's docstring.",
        ),
    ] = True

    show_docstring_raises: Annotated[
        bool,
        _Field(
            group="docstrings",
            description="Whether to display the 'Raises' section in the object's docstring.",
        ),
    ] = True

    show_docstring_receives: Annotated[
        bool,
        _Field(
            group="docstrings",
            description="Whether to display the 'Receives' section in the object's docstring.",
        ),
    ] = True

    show_docstring_returns: Annotated[
        bool,
        _Field(
            group="docstrings",
            description="Whether to display the 'Returns' section in the object's docstring.",
        ),
    ] = True

    show_docstring_type_aliases: Annotated[
        bool,
        _Field(
            group="docstrings",
            description="Whether to display the 'Type Aliases' section in the object's docstring.",
        ),
    ] = True

    show_docstring_type_parameters: Annotated[
        bool,
        _Field(
            group="docstrings",
            description="Whether to display the 'Type Parameters' section in the object's docstring.",
        ),
    ] = True

    show_docstring_warns: Annotated[
        bool,
        _Field(
            group="docstrings",
            description="Whether to display the 'Warns' section in the object's docstring.",
        ),
    ] = True

    show_docstring_yields: Annotated[
        bool,
        _Field(
            group="docstrings",
            description="Whether to display the 'Yields' section in the object's docstring.",
        ),
    ] = True

    show_if_no_docstring: Annotated[
        bool,
        _Field(
            group="docstrings",
            description="Show the object heading even if it has no docstring or children with docstrings.",
        ),
    ] = False

    show_inheritance_diagram: Annotated[
        bool,
        _Field(
            group="docstrings",
            description="Show the inheritance diagram of a class using Mermaid.",
        ),
    ] = False

    show_labels: Annotated[
        bool,
        _Field(
            group="docstrings",
            description="Whether to show labels of the members.",
        ),
    ] = True

    show_object_full_path: Annotated[
        bool,
        _Field(
            group="docstrings",
            description="Show the full Python path of every object.",
        ),
    ] = False

    show_root_full_path: Annotated[
        bool,
        _Field(
            group="docstrings",
            description="Show the full Python path for the root object heading.",
        ),
    ] = True

    show_root_heading: Annotated[
        bool,
        _Field(
            group="headings",
            description="""Show the heading of the object at the root of the documentation tree.

            The root object is the object referenced by the identifier after `:::`.
            """,
        ),
    ] = False

    show_root_members_full_path: Annotated[
        bool,
        _Field(
            group="headings",
            description="Show the full Python path of the root members.",
        ),
    ] = False

    show_root_toc_entry: Annotated[
        bool,
        _Field(
            group="headings",
            description="If the root heading is not shown, at least add a ToC entry for it.",
        ),
    ] = True

    show_signature_annotations: Annotated[
        bool,
        _Field(
            group="signatures",
            description="Show the type annotations in methods and functions signatures.",
        ),
    ] = False

    show_signature_type_parameters: Annotated[
        bool,
        _Field(
            group="signatures",
            description="Show the type parameters in generic classes, methods, functions and type aliases signatures.",
        ),
    ] = False

    show_signature: Annotated[
        bool,
        _Field(
            group="signatures",
            description="Show methods and functions signatures.",
        ),
    ] = True

    show_source: Annotated[
        bool,
        _Field(
            group="general",
            description="Show the source code of this object.",
        ),
    ] = True

    show_submodules: Annotated[
        bool,
        _Field(
            group="members",
            description="When rendering a module, show its submodules recursively.",
        ),
    ] = False

    show_symbol_type_heading: Annotated[
        bool,
        _Field(
            group="headings",
            description="Show the symbol type in headings (e.g. mod, class, meth, func and attr).",
        ),
    ] = False

    show_symbol_type_toc: Annotated[
        bool,
        _Field(
            group="headings",
            description="Show the symbol type in the Table of Contents (e.g. mod, class, methd, func and attr).",
        ),
    ] = False

    skip_local_inventory: Annotated[
        bool,
        _Field(
            group="general",
            description="Whether to prevent objects from being registered in the local objects inventory.",
        ),
    ] = False

    signature_crossrefs: Annotated[
        bool,
        _Field(
            group="signatures",
            description="Whether to render cross-references for type annotations in signatures.",
        ),
    ] = False

    summary: Annotated[
        bool | SummaryOption,
        _Field(
            group="members",
            description="Whether to render summaries of modules, classes, functions (methods) and attributes.",
        ),
    ] = field(default_factory=SummaryOption)

    toc_label: Annotated[
        str,
        _Field(
            group="headings",
            description="A custom string to override the autogenerated toc label of the root object.",
        ),
    ] = ""

    type_parameter_headings: Annotated[
        bool,
        _Field(
            group="headings",
            description="Whether to render headings for type parameters (therefore showing type parameters in the ToC).",
        ),
    ] = False

    unwrap_annotated: Annotated[
        bool,
        _Field(
            group="signatures",
            description="Whether to unwrap `Annotated` types to show only the type without the annotations.",
        ),
    ] = False

    extra: Annotated[
        dict[str, Any],
        _Field(
            group="general",
            description="Extra options.",
        ),
    ] = field(default_factory=dict)

    @classmethod
    def _extract_extra(cls, data: dict[str, Any]) -> tuple[dict[str, Any], dict[str, Any]]:
        field_names = {field.name for field in fields(cls)}
        copy = data.copy()
        return {name: copy.pop(name) for name in data if name not in field_names}, copy

    @classmethod
    def coerce(cls, **data: Any) -> MutableMapping[str, Any]:
        """Coerce data."""
        if "docstring_options" in data:
            docstring_style = data.get("docstring_style", "google")
            docstring_options = data["docstring_options"]
            if docstring_options is not None:
                if docstring_style == "auto":
                    docstring_options = AutoStyleOptions.from_data(**docstring_options)
                elif docstring_style == "google":
                    docstring_options = GoogleStyleOptions(**docstring_options)
                elif docstring_style == "numpy":
                    docstring_options = NumpyStyleOptions(**docstring_options)
                elif docstring_style == "sphinx":
                    docstring_options = SphinxStyleOptions(**docstring_options)
            data["docstring_options"] = docstring_options
        if "summary" in data:
            summary = data["summary"]
            if summary is True:
                summary = SummaryOption(attributes=True, functions=True, classes=True, modules=True, type_aliases=True)
            elif summary is False:
                summary = SummaryOption(
                    attributes=False,
                    functions=False,
                    classes=False,
                    modules=False,
                    type_aliases=False,
                )
            else:
                summary = SummaryOption(**summary)
            data["summary"] = summary
        return data

    @classmethod
    def from_data(cls, **data: Any) -> Self:
        """Create an instance from a dictionary."""
        return cls(**cls.coerce(**data))


@dataclass(frozen=True, kw_only=True)
class PythonOptions(PythonInputOptions):  # type: ignore[override,unused-ignore]
    """Final options passed as template context."""

    filters: list[tuple[re.Pattern, bool]] | Literal["public"] = field(  # type: ignore[assignment]
        default_factory=lambda: [
            (re.compile(filtr.removeprefix("!")), filtr.startswith("!")) for filtr in _DEFAULT_FILTERS
        ],
    )
    """A list of filters, or `"public"`."""

    summary: SummaryOption = field(default_factory=SummaryOption)
    """Whether to render summaries of modules, classes, functions (methods), attributes and type aliases."""

    @classmethod
    def coerce(cls, **data: Any) -> MutableMapping[str, Any]:
        """Create an instance from a dictionary."""
        if "filters" in data and not isinstance(data["filters"], str):
            # Filters are `None` or a sequence of strings (tests use tuples).
            data["filters"] = [
                (re.compile(filtr.removeprefix("!")), filtr.startswith("!")) for filtr in data["filters"] or ()
            ]
        return super().coerce(**data)


@dataclass(frozen=True, kw_only=True)
class Inventory:
    """An inventory."""

    url: Annotated[
        str,
        _Field(
            parent="inventories",
            description="The URL of the inventory.",
        ),
    ]

    base_url: Annotated[
        str | None,
        _Field(
            parent="inventories",
            description="The base URL of the inventory.",
        ),
    ] = None

    domains: Annotated[
        list[str],
        _Field(
            parent="inventories",
            description="The domains to load from the inventory.",
        ),
    ] = field(default_factory=lambda: ["py"])

    @property
    def _config(self) -> dict[str, Any]:
        return {"base_url": self.base_url, "domains": self.domains}


@dataclass(frozen=True, kw_only=True)
class PythonInputConfig:
    """Python handler configuration."""

    inventories: Annotated[
        list[str | Inventory],
        _Field(description="The inventories to load."),
    ] = field(default_factory=list)

    paths: Annotated[
        list[str],
        _Field(description="The paths in which to search for Python packages."),
    ] = field(default_factory=lambda: ["."])

    load_external_modules: Annotated[
        bool | None,
        _Field(description="Whether to always load external modules/packages."),
    ] = None

    options: Annotated[
        PythonInputOptions,
        _Field(description="Configuration options for collecting and rendering objects."),
    ] = field(default_factory=PythonInputOptions)

    locale: Annotated[
        str | None,
        _Field(
            description="Deprecated. Use mkdocstrings' own `locale` setting instead. The locale to use when translating template strings.",
        ),
    ] = None

    @classmethod
    def coerce(cls, **data: Any) -> MutableMapping[str, Any]:
        """Coerce data."""
        return data

    @classmethod
    def from_data(cls, **data: Any) -> Self:
        """Create an instance from a dictionary."""
        return cls(**cls.coerce(**data))


@dataclass(frozen=True, kw_only=True)
class PythonConfig(PythonInputConfig):  # type: ignore[override,unused-ignore]
    """Python handler configuration."""

    inventories: Annotated[
        list[Inventory],
        _Field(description="The object inventories to load."),
    ] = field(default_factory=list)  # type: ignore[assignment]

    options: Annotated[
        dict[str, Any],
        _Field(description="Configuration options for collecting and rendering objects."),
    ] = field(default_factory=dict)  # type: ignore[assignment]

    @classmethod
    def coerce(cls, **data: Any) -> MutableMapping[str, Any]:
        """Coerce data."""
        if "inventories" in data:
            data["inventories"] = [
                Inventory(url=inv) if isinstance(inv, str) else Inventory(**inv) for inv in data["inventories"]
            ]
        return data
