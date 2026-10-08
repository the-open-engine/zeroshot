"""Behavioral tests for documentation publication and migration."""

from __future__ import annotations

import importlib.util
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]


def load_script(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / "scripts" / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


versions = load_script("docs_versions")
hook = load_script("docs_hook")
COMMIT = "a" * 40


class MigrationTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.entries = []

    def snapshot(self, version, aliases=()):
        directory = self.root / version
        directory.mkdir()
        product = None if version == "dev" else version[1:]
        manifest = {
            "schemaVersion": 1,
            "docsVersion": version,
            "productVersion": product,
            "pythonSdkVersion": f"{product}.post1" if product else None,
            "sourceCommit": COMMIT,
            "routes": {"overview": "", "cli": "reference/cli/"},
        }
        versions.write_json(directory / "manifest.json", manifest)
        for page in ("index.html", "reference/cli/index.html"):
            destination = directory / page
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_text(
                f'<link rel="canonical" href="https://example.com/docs/{version}/{page}">'
                f"<p>{version} source content</p>"
                '<script>{"version": {"default": "stable", "alias": true}}</script>',
                encoding="utf-8",
            )
        (directory / "large-asset.bin").write_bytes(b"asset")
        self.entries.append(
            {"version": version, "title": version, "aliases": list(aliases)}
        )
        versions.write_json(self.root / "versions.json", self.entries)

    def test_keeps_newest_patch_numerically_and_redirects_old_pages(self):
        self.snapshot("dev")
        self.snapshot("v10.2.9")
        self.snapshot("v10.2.10", ("stable",))
        self.snapshot("v10.1.0")
        versions.migrate(self.root)
        entries = json.loads((self.root / "versions.json").read_text())
        self.assertEqual(
            [entry["version"] for entry in entries], ["current", "v10.2", "v10.1"]
        )
        self.assertEqual(entries[0]["aliases"], ["dev"])
        self.assertEqual(entries[1]["aliases"], ["stable"])
        manifest = json.loads((self.root / "v10.2/manifest.json").read_text())
        self.assertEqual(manifest["schemaVersion"], 2)
        self.assertEqual(manifest["docsVersion"], "v10.2")
        self.assertEqual(manifest["productVersion"], "10.2.10")
        self.assertEqual(manifest["sourceCommit"], COMMIT)
        page = (self.root / "v10.2/index.html").read_text()
        self.assertIn("v10.2.10 source content", page)
        self.assertIn("https://example.com/docs/v10.2/index.html", page)
        self.assertIn('"default": "current"', page)
        self.assertIn('"alias": false', page)
        for old in ("v10.2.9", "v10.2.10"):
            self.assertFalse((self.root / old / "large-asset.bin").exists())
            redirect = (self.root / old / "reference/cli/index.html").read_text()
            self.assertIn("../../../v10.2/reference/cli/", redirect)
            self.assertIn("location.search + location.hash", redirect)
            self.assertEqual(
                json.loads((self.root / old / "manifest.json").read_text()),
                {
                    "schemaVersion": 2,
                    "redirect": "../v10.2/manifest.json",
                },
            )

    def test_migration_is_idempotent(self):
        self.snapshot("dev")
        self.snapshot("v10.2.0")
        versions.migrate(self.root)
        before = {
            str(path): path.read_bytes()
            for path in self.root.rglob("*")
            if path.is_file()
        }
        versions.migrate(self.root)
        after = {
            str(path): path.read_bytes()
            for path in self.root.rglob("*")
            if path.is_file()
        }
        self.assertEqual(before, after)

    def test_invalid_source_identity_does_not_remove_snapshots(self):
        self.snapshot("v10.2.0")
        self.snapshot("v10.2.1")
        manifest_path = self.root / "v10.2.1/manifest.json"
        manifest = json.loads(manifest_path.read_text())
        manifest["sourceCommit"] = "unknown"
        versions.write_json(manifest_path, manifest)
        with self.assertRaisesRegex(ValueError, "Invalid legacy snapshot"):
            versions.migrate(self.root)
        self.assertTrue((self.root / "v10.2.0/large-asset.bin").exists())
        self.assertTrue((self.root / "v10.2.1/large-asset.bin").exists())


GITHUB_PAGES = "https://the-open-engine.github.io/zeroshot/"
ZEROSHOT_SH = "https://zeroshot.sh/docs/"


class CanonicalTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        versions.write_json(
            self.root / "versions.json",
            [
                {"version": "current", "title": "Current", "aliases": ["dev"]},
                {"version": "v10.3", "title": "v10.3", "aliases": ["stable"]},
                {"version": "v10.2", "title": "v10.2", "aliases": []},
            ],
        )

    def write(self, relative, text):
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")

    def page(self, version, route, origin=GITHUB_PAGES):
        url = f"{origin}{version}/{route}"
        self.write(
            f"{version}/{route}index.html",
            "<html><head>"
            f'<link rel="canonical" href="{url}">'
            f'<meta property="og:url" content="{url}">'
            "</head><body>"
            f'<p>Served from <a href="{GITHUB_PAGES}">{GITHUB_PAGES}</a>.</p>'
            "</body></html>",
        )

    def read(self, relative):
        return (self.root / relative).read_text(encoding="utf-8")

    def canonical(self, relative):
        return versions.CANONICAL_LINK.search(self.read(relative))[1]

    def files(self):
        return {
            str(path): path.read_bytes()
            for path in self.root.rglob("*")
            if path.is_file()
        }

    def test_minor_pages_credit_current_or_their_own_zeroshot_sh_url(self):
        for route in ("", "getting-started/install/"):
            self.page("current", route, ZEROSHOT_SH)
            self.page("v10.2", route)
            # Releases built after site_url changed already name zeroshot.sh.
            self.page("v10.3", route, ZEROSHOT_SH)
        self.page("v10.2", "guides/retired/")
        self.write(
            "v10.2/sitemap.xml",
            f"<urlset><url><loc>{GITHUB_PAGES}v10.2/guides/retired/</loc></url></urlset>",
        )
        self.write("v10.2/sitemap.xml.gz", "stale compressed sitemap")
        stub = (
            '<html><head><link rel="canonical" href="../../../v10.2/guides/retired/">'
            "</head><body></body></html>"
        )
        self.write("v10.2.0/guides/retired/index.html", stub)
        self.write("stable/index.html", "<html><head></head><body>v10.3/</body></html>")
        untouched = {
            path: data
            for path, data in self.files().items()
            if Path(path).relative_to(self.root).parts[0] in {"current", "stable"}
        }

        versions.migrate(self.root)

        for version in ("v10.2", "v10.3"):
            with self.subTest(version=version):
                self.assertEqual(
                    self.canonical(f"{version}/getting-started/install/index.html"),
                    f"{ZEROSHOT_SH}current/getting-started/install/",
                )
                self.assertEqual(
                    self.canonical(f"{version}/index.html"), f"{ZEROSHOT_SH}current/"
                )
        retired = self.read("v10.2/guides/retired/index.html")
        self.assertEqual(
            self.canonical("v10.2/guides/retired/index.html"),
            f"{ZEROSHOT_SH}v10.2/guides/retired/",
        )
        self.assertIn(
            f'<meta property="og:url" content="{ZEROSHOT_SH}v10.2/guides/retired/">',
            retired,
        )
        self.assertIn(f'<a href="{GITHUB_PAGES}">{GITHUB_PAGES}</a>', retired)
        self.assertEqual(
            self.read("v10.2/sitemap.xml"),
            f"<urlset><url><loc>{ZEROSHOT_SH}v10.2/guides/retired/</loc></url></urlset>",
        )
        self.assertFalse((self.root / "v10.2/sitemap.xml.gz").exists())
        self.assertEqual(self.read("v10.2.0/guides/retired/index.html"), stub)
        for path, data in untouched.items():
            self.assertEqual(Path(path).read_bytes(), data, path)

        migrated = self.files()
        versions.migrate(self.root)
        self.assertEqual(self.files(), migrated)

    def test_canonical_follows_current_when_it_drops_a_page(self):
        self.page("current", "guides/acp/", ZEROSHOT_SH)
        self.page("v10.2", "guides/acp/")
        versions.migrate(self.root)
        self.assertEqual(
            self.canonical("v10.2/guides/acp/index.html"),
            f"{ZEROSHOT_SH}current/guides/acp/",
        )
        (self.root / "current/guides/acp/index.html").unlink()
        versions.migrate(self.root)
        self.assertEqual(
            self.canonical("v10.2/guides/acp/index.html"),
            f"{ZEROSHOT_SH}v10.2/guides/acp/",
        )

    def test_site_url_matches_mkdocs_configuration(self):
        configuration = (ROOT / "mkdocs.yml").read_text(encoding="utf-8")
        self.assertIn(f"\nsite_url: {versions.SITE_URL}\n", configuration)


class PublicationTests(unittest.TestCase):
    def test_minor_updates_allow_forward_progress_and_identical_retries(self):
        manifest = {
            "docsVersion": "v10.2",
            "productVersion": "10.2.8",
            "sourceCommit": COMMIT,
        }
        versions.check_update(manifest, "v10.2.9", "b" * 40)
        versions.check_update(manifest, "v10.2.8", COMMIT)
        for version, commit, message in (
            ("v10.2.7", COMMIT, "roll documentation back"),
            ("v10.2.8", "b" * 40, "different source commit"),
            ("v10.3.0", COMMIT, "another minor version"),
        ):
            with self.subTest(version=version, commit=commit):
                with self.assertRaisesRegex(ValueError, message):
                    versions.check_update(manifest, version, commit)

    def test_current_manifest_does_not_claim_a_release(self):
        with patch.dict(os.environ, {"ZEROSHOT_DOCS_COMMIT": COMMIT}, clear=True):
            manifest = hook._manifest()
        self.assertEqual(manifest["docsVersion"], "current")
        self.assertIsNone(manifest["productVersion"])
        self.assertIsNone(manifest["pythonSdkVersion"])

    def test_minor_manifest_records_exact_source_and_publisher(self):
        env = {
            "ZEROSHOT_DOCS_VERSION": "v10.2",
            "ZEROSHOT_PRODUCT_DOCS_VERSION": "10.2.8",
            "ZEROSHOT_DOCS_COMMIT": COMMIT,
            "ZEROSHOT_DOCS_PUBLISHER_COMMIT": "b" * 40,
        }
        with patch.dict(os.environ, env, clear=True):
            manifest = hook._manifest()
        self.assertEqual(manifest["schemaVersion"], 2)
        self.assertEqual(manifest["productVersion"], "10.2.8")
        self.assertEqual(manifest["pythonSdkVersion"], "10.2.8.post1")
        self.assertEqual(manifest["sourceCommit"], COMMIT)
        self.assertEqual(manifest["publisherCommit"], "b" * 40)

    def test_page_routes_resolve_to_documentation_pages(self):
        with patch.dict(os.environ, {"ZEROSHOT_DOCS_COMMIT": COMMIT}, clear=True):
            routes = hook._manifest()["routes"]
        self.assertEqual(routes["runtimePlanReference"], "reference/runtime-plan/")
        self.assertEqual(routes["reviewLoop"], "guides/review-loop/")
        for name, route in routes.items():
            if route.endswith(".json"):
                continue
            with self.subTest(route=name):
                page = route.rstrip("/") or "index"
                self.assertTrue(
                    (ROOT / "docs" / f"{page}.md").is_file()
                    or (ROOT / "docs" / page / "index.md").is_file(),
                    f"route {name} has no documentation page",
                )

    def test_routes_to_pages_missing_from_the_build_are_dropped(self):
        with tempfile.TemporaryDirectory() as directory:
            site = Path(directory)
            (site / "guides/observe-and-control").mkdir(parents=True)
            (site / "guides/observe-and-control/index.html").write_text("page")
            (site / "index.html").write_text("page")
            (site / "reference/cluster").mkdir(parents=True)
            (site / "reference/cluster/schema.json").write_text("{}")
            with patch.dict(os.environ, {"ZEROSHOT_DOCS_COMMIT": COMMIT}, clear=True):
                routes = hook._manifest(site)["routes"]
        self.assertEqual(
            routes,
            {
                "overview": "",
                "runControl": "guides/observe-and-control/",
                "schema": "reference/cluster/schema.json",
            },
        )

    def test_rejects_mismatched_minor_and_product_identity(self):
        for docs, product in (
            ("v10.2", "10.3.0"),
            ("current", "10.2.8"),
            ("v10.2", ""),
        ):
            with self.subTest(docs=docs, product=product):
                with patch.dict(
                    os.environ,
                    {
                        "ZEROSHOT_DOCS_VERSION": docs,
                        "ZEROSHOT_PRODUCT_DOCS_VERSION": product,
                    },
                    clear=True,
                ):
                    with self.assertRaises(ValueError):
                        hook._manifest()


if __name__ == "__main__":
    unittest.main()
