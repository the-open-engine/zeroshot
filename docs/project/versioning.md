# Documentation versions

**Current** follows `main` and is the default. It may describe changes that have not shipped yet.
For an installed release, choose its minor version from the header. Each minor version documents
its newest published patch: releases `10.2.0` through `10.2.8` share `v10.2/`, whose content comes
from `10.2.8` after that release publishes.

Patch releases update the existing minor documentation without adding another selector entry.
Older minor versions remain available.

## URL contract

The docs base is `https://the-open-engine.github.io/zeroshot/`. Resolve these paths against that
base, including its `/zeroshot/` project prefix:

| Relative path                                 | Meaning                                                          |
| --------------------------------------------- | ---------------------------------------------------------------- |
| `current/`                                    | Current `main` documentation; the site root redirects here       |
| `vX.Y/`                                       | Newest published patch within that minor version                 |
| `versions.json`                               | Selector entries: Current and minor versions                     |
| `current/manifest.json`, `vX.Y/manifest.json` | Source identity and logical routes                               |
| `dev/`                                        | Legacy page redirects to Current                                 |
| `stable/`                                     | Legacy page redirects to the newest released minor               |
| `vX.Y.Z/`                                     | Previously published patch pages redirect to their minor version |

The selector does not list legacy aliases. Existing patch links retain their page, query string,
and fragment, but now show the minor version's newest patch. New releases do not create patch paths.

## Build manifest

Manifest schema 2 distinguishes a docs channel from the exact product used to generate its APIs:

```json
{
  "schemaVersion": 2,
  "docsVersion": "v10.2",
  "productVersion": "10.2.8",
  "pythonSdkVersion": "10.2.8.post1",
  "sourceCommit": "FULL_PRODUCT_SOURCE_COMMIT",
  "publisherCommit": "FULL_PUBLICATION_WORKFLOW_COMMIT",
  "routes": {
    "overview": "",
    "runtimePlan": "concepts/runtimes-and-connections/",
    "runControl": "guides/observe-and-control/",
    "cli": "zeroshot-cli/",
    "pythonApi": "reference/python/",
    "clusterApi": "reference/cluster/api/"
  }
}
```

The example abbreviates `routes`; builds retain the complete logical route map, including protocol
schemas. Values are relative to the selected version root. Current uses `docsVersion: current`
and null product and Python SDK versions; its source commit identifies `main` at build time.

Publication tools and this version-policy page can come from a newer workflow commit than the
product source. `publisherCommit` records that distinction. Minor pages migrated from the old site
retain their original source identity and omit `publisherCommit` until rebuilt; their existing
rendered product content is reused.

Old patch manifests now contain a schema-2 redirect record such as
`{"schemaVersion": 2, "redirect": "../v10.2/manifest.json"}`. This is JSON metadata, not an HTTP
redirect: GitHub Pages cannot redirect JSON clients with its HTML redirect pages. Clients that need
an exact historical patch manifest must use the site's Git history. Do not treat a minor manifest
as proof that the docs match an older patch exactly.

## Linking from Zeroshot Cloud

For a deployed product version `X.Y.Z`, fetch `vX.Y/manifest.json` and require schema 2. Confirm that
`productVersion` belongs to the deployed major/minor line, then resolve a logical route against
`vX.Y/`. For example, `10.2.3` uses the `v10.2` docs, whose manifest may identify `10.2.8`.
Callers should label this as minor-version documentation and account for features added after their
installed patch. Compare source commits only when the documented and deployed product versions match.

If the minor has no docs, offer an explicitly labelled Current link. Current may contain unreleased
behavior. Cloud owns instructions for accounts, organization policy, OAuth, GitHub Apps, and capacity;
core graph, runtime, CLI, protocol, and SDK material stays here.

## Publication and recovery

Every `main` publication updates Current. After Python SDK revision 1, a product release updates its
minor documentation from the exact release tag and commit. A retry cannot replace a newer patch with
an older one or substitute a different source for the same product version. Later Python-only
revisions do not rebuild product docs.

To retry, run **Publish versioned documentation**. Omit inputs for Current, or supply the exact
`vX.Y.Z` release and source commit for a minor. The optional `stable` input maintains the legacy
released-docs redirect; it never changes the site's Current default.

The publisher consolidates existing patch snapshots before publishing, retaining the newest patch
per minor and replacing old pages with redirects. It stages the complete result before pushing
`gh-pages` and deploying through GitHub Pages Actions. No release tags or binary artifacts change.
If a release is the first publication, the publisher first builds Current from `main` in an isolated
checkout, then publishes the requested minor from its release source. Both become visible together.
