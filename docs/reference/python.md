# Python API

The public package is `zeroshot`; the install distribution is `the-open-engine-zeroshot`. All I/O
methods are asynchronous. Public data objects are frozen dataclasses, and exact protocol documents
are copied defensively at their boundary.

The reference is generated from the curated exports in `zeroshot.__init__` and their source
docstrings:

- [Client and run handle](python/client.md): submission, lookup, watch, logs, and stop operations.
- [Configuration](python/configuration.md): targets, presets, graph inputs, and runtime plans.
- [Result models](python/results.md): immutable status, log, source, and terminal values.
- [Exceptions](python/errors.md): request, target, protocol, timeout, and failed-run errors.

See the [Python SDK guide](../guides/python-sdk.md) for complete submissions and target examples.

## Version identity

The SDK release paired with Zeroshot `vX.Y.Z` is `X.Y.Z.post1`. This site records that value in its
version manifest. If a later SDK-only revision is published, it does not alter an existing core docs
snapshot.
