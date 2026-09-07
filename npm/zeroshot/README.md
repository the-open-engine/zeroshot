# `@the-open-engine-company/zeroshot`

Thin installer for the canonical `zeroshot` executable. The package selects the release archive for the current Node platform and architecture, verifies it against that release's `SHA256SUMS`, and installs only the verified executable.

Installation fails closed with `UNSUPPORTED_ZEROSHOT_HOST` when the host has no declared release target. Source compilation and cross-target substitution are not supported.

Supported hosts are Linux x64/arm64, macOS x64/arm64, and Windows x64.
