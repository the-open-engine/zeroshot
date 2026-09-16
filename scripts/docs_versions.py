"""Consolidate published patch documentation and guard minor-version updates."""

from __future__ import annotations

import argparse
import html
import json
import posixpath
import re
import shutil
from pathlib import Path

RELEASE = re.compile(r"^v(\d+)\.(\d+)\.(\d+)$")
MINOR = re.compile(r"^v(\d+)\.(\d+)$")
COMMIT = re.compile(r"^[0-9a-f]{40}$")


def release_parts(version: str) -> tuple[int, ...]:
    match = RELEASE.fullmatch(version)
    if match is None or int(match[1]) < 8:
        raise ValueError(f"Invalid canonical release: {version}")
    return tuple(int(part) for part in match.groups())


def check_update(manifest: dict, version: str, commit: str) -> None:
    """Refuse patch rollback and same-version source substitution."""
    requested = release_parts(version)
    if not COMMIT.fullmatch(commit):
        raise ValueError("Release source must be a full lowercase Git commit")
    published = release_parts("v" + manifest["productVersion"])
    expected_minor = ".".join(version.split(".")[:2])
    if manifest.get("docsVersion") != expected_minor or requested[:2] != published[:2]:
        raise ValueError("Published manifest belongs to another minor version")
    if requested < published:
        raise ValueError(
            f"Refusing to roll documentation back from {published} to {requested}"
        )
    if requested == published and manifest.get("sourceCommit") != commit:
        raise ValueError(
            "The same product release cannot have a different source commit"
        )


def write_json(path: Path, value: object) -> None:
    path.write_text(
        json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )


def redirect_pages(root: Path, old: str, new: str, pages: list[Path]) -> None:
    """Replace a snapshot with redirects that retain page, query, and fragment."""
    shutil.rmtree(root / old)
    for page in pages:
        destination = root / old / page
        destination.parent.mkdir(parents=True, exist_ok=True)
        relative = posixpath.relpath(
            f"{new}/{page.as_posix()}", f"{old}/{page.parent.as_posix()}"
        )
        if relative.endswith("/index.html"):
            relative = relative.removesuffix("index.html")
        escaped = html.escape(relative, quote=True)
        destination.write_text(
            '<!doctype html><html><head><meta charset="utf-8">'
            f'<meta http-equiv="refresh" content="0; url={escaped}">'
            f'<link rel="canonical" href="{escaped}">'
            f"<script>location.replace({json.dumps(relative)} + location.search + location.hash);"
            f'</script></head><body><a href="{escaped}">Continue to documentation</a></body></html>\n',
            encoding="utf-8",
        )
    # HTML redirects cannot redirect JSON clients on GitHub Pages. Make the
    # changed contract explicit instead of returning misleading release identity.
    write_json(
        root / old / "manifest.json",
        {
            "schemaVersion": 2,
            "redirect": f"../{new}/manifest.json",
        },
    )


def retarget_snapshot(directory: Path, old: str, new: str) -> None:
    """Update URL metadata while preserving the selected source's content."""
    for path in directory.rglob("*"):
        if path.is_file() and path.suffix in {".html", ".xml", ".json"}:
            original = path.read_text(encoding="utf-8")
            updated = original.replace(f"/{old}/", f"/{new}/")
            updated = updated.replace('"default": "stable"', '"default": "current"')
            updated = updated.replace('"alias": true', '"alias": false')
            if original != updated:
                path.write_text(updated, encoding="utf-8")
    manifest_path = directory / "manifest.json"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    manifest.update(schemaVersion=2, docsVersion=new)
    write_json(manifest_path, manifest)
    # MkDocs also emits a compressed sitemap. Keep the rewritten XML as the
    # authoritative sitemap rather than retaining compressed old patch URLs.
    (directory / "sitemap.xml.gz").unlink(missing_ok=True)


def group_versions(versions: list[dict]) -> tuple[dict, list[dict]]:
    groups: dict[str, list[dict]] = {}
    retained = []
    for entry in versions:
        version = entry["version"]
        if RELEASE.fullmatch(version):
            release_parts(version)
            minor = version.rsplit(".", 1)[0]
            groups.setdefault(minor, []).append(entry)
        elif version in {"current", "dev"} or MINOR.fullmatch(version):
            retained.append(entry)
        else:
            raise ValueError(f"Unexpected published documentation version: {version}")

    return groups, retained


def validate_legacy(root: Path, groups: dict) -> None:
    """Validate every source identity before removing any snapshot."""
    for entries in groups.values():
        for entry in entries:
            version = entry["version"]
            manifest = json.loads(
                (root / version / "manifest.json").read_text(encoding="utf-8")
            )
            if (
                manifest.get("docsVersion") != version
                or manifest.get("productVersion") != version[1:]
                or not COMMIT.fullmatch(manifest.get("sourceCommit", ""))
            ):
                raise ValueError(f"Invalid legacy snapshot identity: {version}")


def consolidate_minor(
    root: Path, minor: str, entries: list[dict], retained: list[dict]
) -> None:
    newest = max(entries, key=lambda entry: release_parts(entry["version"]))
    existing = next((entry for entry in retained if entry["version"] == minor), None)
    if existing is None:
        shutil.copytree(root / newest["version"], root / minor)
        retarget_snapshot(root / minor, newest["version"], minor)
        retained.append({"version": minor, "title": minor, "aliases": []})
    else:
        manifest = json.loads(
            (root / minor / "manifest.json").read_text(encoding="utf-8")
        )
        if release_parts("v" + manifest["productVersion"]) < release_parts(
            newest["version"]
        ):
            raise ValueError(
                f"Existing {minor} is older than the legacy snapshot; republish it first"
            )
    for entry in entries:
        old = entry["version"]
        pages = [path.relative_to(root / old) for path in (root / old).rglob("*.html")]
        redirect_pages(root, old, minor, pages)


def migrate_current(root: Path, retained: list[dict]) -> None:
    development = next((entry for entry in retained if entry["version"] == "dev"), None)
    if development is not None:
        if any(entry["version"] == "current" for entry in retained):
            raise ValueError(
                "Both dev and current are real versions; resolve before migration"
            )
        shutil.copytree(root / "dev", root / "current")
        retarget_snapshot(root / "current", "dev", "current")
        pages = [
            path.relative_to(root / "dev") for path in (root / "dev").rglob("*.html")
        ]
        redirect_pages(root, "dev", "current", pages)
        development.update(version="current", title="Current", aliases=["dev"])


def migrate_stable(root: Path, retained: list[dict]) -> None:
    """Keep stable links on the newest released minor, outside the selector."""
    minors = [entry for entry in retained if MINOR.fullmatch(entry["version"])]
    if minors:
        newest_minor = max(
            minors, key=lambda entry: tuple(map(int, entry["version"][1:].split(".")))
        )
        for entry in retained:
            entry["aliases"] = [
                alias for alias in entry["aliases"] if alias != "stable"
            ]
        newest_minor["aliases"].append("stable")
        if (root / "stable").exists():
            pages = [
                path.relative_to(root / "stable")
                for path in (root / "stable").rglob("*.html")
            ]
            redirect_pages(root, "stable", newest_minor["version"], pages)


def migrate(root: Path) -> None:
    """Keep the newest patch of each minor and preserve old HTML links."""
    versions_path = root / "versions.json"
    if not versions_path.exists():
        return
    groups, retained = group_versions(
        json.loads(versions_path.read_text(encoding="utf-8"))
    )
    validate_legacy(root, groups)
    for minor, entries in groups.items():
        consolidate_minor(root, minor, entries, retained)
    migrate_current(root, retained)
    # After the legacy migration, Mike owns aliases. Publishing Current must
    # not move the released stable alias as a side effect.
    if groups:
        migrate_stable(root, retained)
    retained.sort(
        key=lambda entry: (
            entry["version"] == "current",
            tuple(map(int, entry["version"][1:].split(".")))
            if MINOR.fullmatch(entry["version"])
            else (),
        ),
        reverse=True,
    )
    write_json(versions_path, retained)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    migration = subparsers.add_parser("migrate")
    migration.add_argument("root", type=Path)
    check = subparsers.add_parser("check-update")
    check.add_argument("manifest", type=Path)
    check.add_argument("version")
    check.add_argument("commit")
    args = parser.parse_args()
    if args.command == "migrate":
        migrate(args.root)
    else:
        check_update(
            json.loads(args.manifest.read_text(encoding="utf-8")),
            args.version,
            args.commit,
        )


if __name__ == "__main__":
    main()
