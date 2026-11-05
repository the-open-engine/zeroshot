from typing import (
    AbstractSet,
    Callable,
    Dict,
    Literal,
    Mapping,
    Optional,
    Set,
    Tuple,
    Union,
)

ALLOWED_TAGS: Set[str]
ALLOWED_ATTRIBUTES: Dict[str, Set[str]]
ALLOWED_URL_SCHEMES: Set[str]
CLEAN_CONTENT_TAGS: Set[str]

UrlRelative = Union[
    Literal["pass_through", "deny"],
    Tuple[Literal["rewrite_with_base"], str],
    Tuple[Literal["rewrite_with_root"], str, str],
    Callable[[str], Optional[str]],
]

class Cleaner:
    def __init__(
        self,
        tags: Optional[AbstractSet[str]] = None,
        clean_content_tags: Optional[AbstractSet[str]] = None,
        attributes: Optional[Mapping[str, AbstractSet[str]]] = None,
        attribute_filter: Optional[Callable[[str, str, str], Optional[str]]] = None,
        strip_comments: bool = True,
        link_rel: Optional[str] = "noopener noreferrer",
        generic_attribute_prefixes: Optional[AbstractSet[str]] = None,
        tag_attribute_values: Optional[Mapping[str, Mapping[str, AbstractSet[str]]]] = None,
        set_tag_attribute_values: Optional[Mapping[str, Mapping[str, str]]] = None,
        url_schemes: Optional[AbstractSet[str]] = None,
        allowed_classes: Optional[Mapping[str, AbstractSet[str]]] = None,
        filter_style_properties: Optional[AbstractSet[str]] = None,
        url_relative: Optional[UrlRelative] = None,
        id_prefix: Optional[str] = None,
    ) -> None: ...
    def clean(self, html: str) -> str: ...

def clean(
    html: str,
    tags: Optional[AbstractSet[str]] = None,
    clean_content_tags: Optional[AbstractSet[str]] = None,
    attributes: Optional[Mapping[str, AbstractSet[str]]] = None,
    attribute_filter: Optional[Callable[[str, str, str], Optional[str]]] = None,
    strip_comments: bool = True,
    link_rel: Optional[str] = "noopener noreferrer",
    generic_attribute_prefixes: Optional[AbstractSet[str]] = None,
    tag_attribute_values: Optional[Mapping[str, Mapping[str, AbstractSet[str]]]] = None,
    set_tag_attribute_values: Optional[Mapping[str, Mapping[str, str]]] = None,
    url_schemes: Optional[AbstractSet[str]] = None,
    allowed_classes: Optional[Mapping[str, AbstractSet[str]]] = None,
    filter_style_properties: Optional[AbstractSet[str]] = None,
    url_relative: Optional[UrlRelative] = None,
    id_prefix: Optional[str] = None,
) -> str: ...
def clean_text(html: str, tags: Optional[AbstractSet[str]] = None) -> str: ...
def escape(html: str, tags: Optional[AbstractSet[str]] = None) -> str: ...
def is_html(html: str) -> bool: ...
