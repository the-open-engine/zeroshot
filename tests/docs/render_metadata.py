"""Render metadata with the documentation dependencies installed by docs CI."""

import json
import unittest
from html.parser import HTMLParser
from pathlib import Path
from types import SimpleNamespace

from jinja2 import ChoiceLoader, DictLoader, Environment, FileSystemLoader


class HeadParser(HTMLParser):
    def __init__(self):
        super().__init__()
        self.metadata = {}
        self.structured_data = []
        self.in_json = False

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if tag == "meta":
            self.metadata[attrs.get("property", attrs.get("name"))] = attrs["content"]
        if tag == "script" and attrs.get("type") == "application/ld+json":
            self.in_json = True

    def handle_endtag(self, tag):
        if tag == "script":
            self.in_json = False

    def handle_data(self, data):
        if self.in_json:
            self.structured_data.append(json.loads(data))


class MetadataTests(unittest.TestCase):
    def test_versioned_social_metadata_and_breadcrumbs_escape_text(self):
        templates = Path(__file__).resolve().parents[2] / "docs/overrides"
        environment = Environment(
            loader=ChoiceLoader(
                [
                    FileSystemLoader(templates),
                    DictLoader({"base.html": "{% block extrahead %}{% endblock %}"}),
                ]
            )
        )
        title = 'Run "reviews" & <checks>'
        description = 'A "quoted" description with </script> & details.'
        for version in ("current", "v10.2"):
            for trailing_slash in ("", "/"):
                with self.subTest(version=version, trailing_slash=trailing_slash):
                    root = f"https://zeroshot.sh/docs/{version}"
                    canonical = root + "/guides/review-loop/"
                    rendered = environment.get_template("main.html").render(
                        page=SimpleNamespace(
                            title=title,
                            meta={"description": description},
                            canonical_url=canonical,
                            is_homepage=False,
                        ),
                        config=SimpleNamespace(site_url=root + trailing_slash),
                    )
                    parsed = HeadParser()
                    parsed.feed(rendered)
                    for key in ("og:title", "twitter:title"):
                        self.assertEqual(
                            parsed.metadata[key], title + " | zeroshot docs"
                        )
                    for key in ("og:description", "twitter:description"):
                        self.assertEqual(parsed.metadata[key], description)
                    self.assertEqual(parsed.metadata["og:url"], canonical)
                    self.assertEqual(
                        parsed.metadata["twitter:card"], "summary_large_image"
                    )
                    self.assertEqual(len(parsed.structured_data), 1)
                    breadcrumbs = parsed.structured_data[0]
                    self.assertEqual(breadcrumbs["@type"], "BreadcrumbList")
                    items = breadcrumbs["itemListElement"]
                    self.assertEqual(items[0]["item"], root + "/")
                    self.assertEqual(items[1]["item"], canonical)
                    self.assertEqual(items[1]["name"], title)


if __name__ == "__main__":
    unittest.main()
