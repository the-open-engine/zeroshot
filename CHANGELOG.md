# Changelog

## 8.0.0

- Replaced the former Node.js product with the native `zeroshot` executable as the only canonical
  runtime and CLI.
- Moved the public npm package to `@the-open-engine-company/zeroshot` as a verified binary delivery
  package.
- Standardized releases on `vX.Y.Z`, `zeroshot-vX.Y.Z-<target>.tar.gz`, and
  `ghcr.io/the-open-engine/zeroshot-target`.
- Renamed the Python distribution to `the-open-engine-zeroshot` while preserving `import zeroshot`.
- Removed legacy commands, configuration, state compatibility, migration, aliases, and the former
  Node release train.

Historical pre-v8 releases remain available through Git tags and GitHub Releases. Their interfaces
are not supported by v8.
