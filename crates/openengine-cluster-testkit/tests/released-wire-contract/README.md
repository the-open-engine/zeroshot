# Released wire-contract control

This isolated Cargo workspace uses unmodified client, protocol and server crates from Zeroshot
v17.1.1 (`15e17f792359cf6f6a0358918ad6146da57fb32f`) and actual pre-workspace main
(`79ccf9effe15a47cf44b893421320816f8c1ce6f`), which already supports run parts and artifacts. Keeping its lockfile separate avoids ambiguous
package selectors in the product workspace. No provider credentials or live gateway are needed.

From the repository root:

```sh
cargo test --locked --manifest-path crates/openengine-cluster-testkit/tests/released-wire-contract/Cargo.toml --target-dir target --jobs 1
```

The controls exercise version rejection at NDJSON and WebSocket initialization in both directions.
Actual-main discovery identifiers also require bidirectional direct/hosted upgrades. Its strict
status/list/watch readers reject workspace data while accepting the same payloads with only
`workspace` removed; part identity remains intact. Same-version readers retain workspace fields.
A stored continuation from the release round-trips through the current reader with its original ID
and predecessor.

The protocol's `native_v2_observation` integration suite invokes these controls together with the
current runtime and HTTP wire-contract tests. That existing suite runs in the Linux, Windows and
macOS CI lanes. Child Cargo commands select only the leaf tests and use one build worker in the
`wire-contract/current` and `wire-contract/released` subdirectories of the Cargo target directory.
This preserves the surrounding job's CLI feature profile and keeps each Cargo workspace's build
artifacts independent. They never select the parent observation suite.
The Windows test script prebuilds the leaf tests before its bounded native execution host starts.
Child commands clear parent-package Cargo metadata so build scripts reuse those prebuilds.
