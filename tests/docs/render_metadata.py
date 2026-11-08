"""Render metadata with the documentation dependencies installed by docs CI."""

import json
import unittest
import xml.etree.ElementTree as ET
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
        self.in_title = False
        self.title = ""

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if tag == "meta":
            self.metadata[attrs.get("property", attrs.get("name"))] = attrs["content"]
        if tag == "script" and attrs.get("type") == "application/ld+json":
            self.in_json = True
        if tag == "title":
            self.in_title = True

    def handle_endtag(self, tag):
        if tag == "script":
            self.in_json = False
        if tag == "title":
            self.in_title = False

    def handle_data(self, data):
        if self.in_json:
            self.structured_data.append(json.loads(data))
        if self.in_title:
            self.title += data


class MetadataTests(unittest.TestCase):
    def setUp(self):
        templates = Path(__file__).resolve().parents[2] / "docs/overrides"
        self.environment = Environment(
            loader=ChoiceLoader(
                [
                    FileSystemLoader(templates),
                    DictLoader(
                        {
                            "base.html": "{% block htmltitle %}{% endblock %}"
                            "{% block extrahead %}{% endblock %}"
                        }
                    ),
                ]
            )
        )

    def test_versioned_social_metadata_and_breadcrumbs_escape_text(self):
        title = 'Run "reviews" & <checks>'
        description = 'A "quoted" description with </script> & details.'
        for version in ("current", "v10.2"):
            for trailing_slash in ("", "/"):
                with self.subTest(version=version, trailing_slash=trailing_slash):
                    root = f"https://zeroshot.sh/docs/{version}"
                    canonical = root + "/guides/review-loop/"
                    rendered = self.environment.get_template("main.html").render(
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
                    self.assertEqual(parsed.title, title + " | zeroshot docs")
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

    def test_homepage_and_authored_titles_match_search_and_social_previews(self):
        for homepage, metadata, expected in (
            (True, {}, "zeroshot documentation | Coding agents and QA workflows"),
            (False, {"title": "Python API"}, "Python API | zeroshot docs"),
        ):
            with self.subTest(homepage=homepage):
                parsed = HeadParser()
                parsed.feed(
                    self.environment.get_template("main.html").render(
                        page=SimpleNamespace(
                            title="Overview",
                            meta=metadata,
                            canonical_url="https://zeroshot.sh/docs/current/",
                            is_homepage=homepage,
                        ),
                        config=SimpleNamespace(
                            site_url="https://zeroshot.sh/docs/current/",
                            site_description="Documentation for coding agents.",
                        ),
                    )
                )
                self.assertEqual(parsed.title, expected)
                self.assertEqual(parsed.metadata["og:title"], expected)
                self.assertEqual(parsed.metadata["twitter:title"], expected)

    def test_missing_page_has_a_documentation_title(self):
        parsed = HeadParser()
        parsed.feed(self.environment.get_template("main.html").render(page=None))
        self.assertEqual(parsed.title, "zeroshot docs")

    def test_sitemap_uses_verified_content_dates_and_omits_unknown_dates(self):
        pages = [
            SimpleNamespace(
                page=SimpleNamespace(
                    is_link=False,
                    canonical_url=f"https://zeroshot.sh/docs/current/{route}/",
                    abs_url=None,
                    update_date="2026-11-08",
                    meta={"sitemap_lastmod": changed},
                )
            )
            for route, changed in (("known", "2026-10-04"), ("unknown", None))
        ]
        rendered = self.environment.get_template("sitemap.xml").render(pages=pages)
        root = ET.fromstring(rendered)
        namespace = {"s": "http://www.sitemaps.org/schemas/sitemap/0.9"}
        urls = root.findall("s:url", namespace)
        self.assertEqual(len(urls), 2)
        self.assertEqual(
            urls[0].findtext("s:lastmod", namespaces=namespace), "2026-10-04"
        )
        self.assertIsNone(urls[1].find("s:lastmod", namespace))


if __name__ == "__main__":
    unittest.main()
