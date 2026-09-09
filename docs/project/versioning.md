# Documentation versions

For every release published through this workflow, the documentation site writes an immutable
snapshot whose pages do not change later.

## URL contract

The initial docs base URL is `https://the-open-engine.github.io/zeroshot/`. Every path in this table
is relative to that base. Keep its `/zeroshot/` project prefix while using GitHub Pages; with another
docs base, resolve the same relative paths against that base instead.

| Relative path          | Meaning                                      | Mutable? |
| ---------------------- | -------------------------------------------- | -------- |
| `vX.Y.Z/`              | Exact canonical product release              | No       |
| `stable/`              | Redirect to the newest canonical release     | Yes      |
| `dev/`                 | Current `main` documentation                 | Yes      |
| `versions.json`        | Version selector data written by Mike        | Yes      |
| `vX.Y.Z/manifest.json` | Machine-readable identity and logical routes | No       |

The site root redirects to `stable` after the first release deployment. Before that, it points to
`dev`.

## Snapshot manifest

The build hook writes this manifest beside the pages:

```json
{
  "docsVersion": "v9.1.0",
  "productVersion": "9.1.0",
  "pythonSdkVersion": "9.1.0.post1",
  "routes": {
    "clusterApi": "reference/cluster/api/",
    "cli": "zeroshot-cli/",
    "execution": "concepts/execution/",
    "graphSpec": "reference/cluster/graph/",
    "install": "getting-started/install/",
    "openrpc": "reference/cluster/openrpc.json",
    "overview": "",
    "portableBindings": "reference/cluster/portable-bindings/",
    "pythonApi": "reference/python/",
    "pythonSdk": "guides/python-sdk/",
    "quickstart": "getting-started/first-run/",
    "runControl": "guides/observe-and-control/",
    "runtimePlan": "concepts/runtimes-and-connections/",
    "schema": "reference/cluster/schema.json",
    "targets": "concepts/targets/"
  },
  "schemaVersion": 1,
  "sourceCommit": "FULL_GIT_COMMIT"
}
```

Route values are relative to the version root. Consumers should reject an unknown
`schemaVersion`, check `productVersion`, and then join the selected route to the exact version base.

## Linking from Zeroshot Cloud

The Cloud deployment already knows the Zeroshot image version and source commit, which is enough to
link to matching core documentation without copying it. Use this sequence:

1. read the deployed image's `X.Y.Z` version;
2. resolve `vX.Y.Z/manifest.json` against the docs base and fetch it once during deployment or
   startup;
3. confirm `productVersion` and, when available, `sourceCommit`;
4. resolve a logical route from the manifest against that same exact-version base.

For example, version `9.1.0` uses
`https://the-open-engine.github.io/zeroshot/v9.1.0/manifest.json`. Joining
`routes.runtimePlan` produces
`https://the-open-engine.github.io/zeroshot/v9.1.0/concepts/runtimes-and-connections/`.

A Cloud runtime-plan page can select `routes.runtimePlan`, while a run page can select
`routes.runControl`. The Cloud site owns instructions for OAuth, GitHub Apps, accounts, organization
policy, and capacity; graph, runtime, CLI, protocol, and SDK material stays here.

Link to these pages instead of importing HTML or placing the site in an iframe. Each snapshot has its
own search index, navigation, canonical URL, and generated anchors, while Cloud avoids serving a copy
of the build output.

## Release and recovery rules

`main` publishes `dev`. After Python SDK revision 1, the product release workflow calls the docs
publisher with the same tag and source commit. The publisher refuses to replace a version path when
its manifest names another commit; rerunning the same source can repair `stable` without rebuilding
the snapshot.

Later Python-only revisions can use a separate `python/X.Y.Z.postN/` namespace when that need first
arises. They must not rebuild `vX.Y.Z/` from a newer commit.

Canonical tags created before this publisher lands do not contain the site source and cannot be
backfilled without misrepresenting their provenance. Cloud should test for the exact manifest and
either hide the core-doc link or label a `dev/` fallback as development documentation. Exact-version
links begin with the first later canonical release.

GitHub Pages hosts the site at `https://the-open-engine.github.io/zeroshot/` with **GitHub Actions**
as its publishing source. If a publication needs to be retried, manually run **Publish versioned
documentation**. Mike keeps the version tree on `gh-pages`, and the publisher uploads that tree
through the official Pages actions; repository owners can add a custom domain later without changing
version or manifest paths.
