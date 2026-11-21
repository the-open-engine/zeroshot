# Released wire-contract control

This isolated Cargo workspace uses unmodified client, protocol and server crates from Zeroshot
v16.0.0 (`df429ea0e36b5baa7eadb997e1a7efe276d95ae1`). Keeping its lockfile separate avoids ambiguous
package selectors in the product workspace. No provider credentials or live gateway are needed.

From the repository root:

```sh
cargo test --locked --manifest-path crates/openengine-cluster-testkit/tests/released-wire-contract/Cargo.toml --target-dir target --jobs 1
```

The controls exercise real NDJSON and WebSocket initialization in both directions. They also decode
current ledger/projector serialization with released direct and hosted readers, proving timing is
incompatible without the gate and removing only timing restores decoding. Product workspace tests
cover matching-contract status, mixed lists, active/finished watches and hosted discovery refusal.

The protocol's `native_v2_observation` integration suite invokes these controls together with the
current runtime and HTTP wire-contract tests. That existing suite runs in the Linux, Windows and
macOS CI lanes. Child Cargo commands select only the leaf tests and use one build worker in the
`wire-contract/current` and `wire-contract/released` subdirectories of the Cargo target directory.
This preserves the surrounding job's CLI feature profile and keeps each Cargo workspace's build
artifacts independent. They never select the parent observation suite.
The Windows test script prebuilds the leaf tests before its bounded native execution host starts.
Child commands clear parent-package Cargo metadata so build scripts reuse those prebuilds.
