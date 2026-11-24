# Target files

`zeroshot file` addresses files and named directory trees on the selected target. Omit `--target`
for local state, or select a registered direct target. The direct target stores its files below
its `--storage` directory; mount that directory on a persistent Docker volume.

```bash
zeroshot file upload ./dataset datasets/example --operation upload-example
zeroshot file list
zeroshot file list datasets/example
zeroshot file download datasets/example ./downloaded
zeroshot file download datasets/example ./sample.bin --member train/sample.bin
zeroshot file remove datasets/example
```

Add `--target NAME` to use that target for each operation. `--scope user` (the default) and
`--scope org` select independent reusable libraries. A named path denotes a complete file or tree;
replacing it publishes one complete selection. Member names retain directories and empty folders.
Downloads require a new destination and publish it only after validating the complete transfer.
Use portable ASCII names; absolute paths, traversal, Windows reserved names, case collisions,
links and special files are rejected. Permissions and executable bits are not transferred.

`file list --run RUN_ID` lists host-managed publications. `file download PATH OUTPUT --run RUN_ID`
reads one. Clients cannot write or remove run publications. The host separately owns staged inputs,
private pins, request publications and final publications. References to files in a live local
workspace are not managed publications and do not appear in this tree.

An upload receipt includes its operation key. Supply `--operation` when retrying a known upload:
identical retries return the original logical result, while conflicting reuse fails. Subsequent
replacements or removals remain effective when an older operation is replayed. Pins keep the
selected bytes private and stable even after a library path is changed or deleted; hosts explicitly
release them when unused.
Managed publication names remain reserved after release so an earlier receipt cannot resolve to
another publication's bytes.

## Transport and host contract

The Rust protocol crate's `target_files` module owns the binary contract and constants. Its
generated `protocol/openengine-cluster/v1/target-files.schema.json` describes metadata.
The independent `extensions.files` marker has kind `openengine.target-files/v1`. An absent or unknown
marker means unsupported; existing targets keep serving their other operations.

All file HTTP operations POST to `/native-v2/files` on the selected target's authenticated origin.
They use `application/vnd.openengine.file-tree.v1`, explicit `Content-Length`, no redirects and
`Cache-Control: no-store` responses. A request starts with a big-endian u32 metadata length and
that many JSON bytes (`FileRequest`). Only `replace` appends a binary tree. List/remove/replace
responses use `FileResponse` JSON; download responses contain a binary tree directly.

A tree starts with the eight bytes `OEFILES1`. Each entry has a big-endian u32 JSON header length
and a `FileEntry` header. A file header is followed by its exact `byteLength` raw bytes and its
32-byte binary SHA-256 digest. Directories have no content. A zero u32 ends the tree and must be
followed by EOF. The first entry names the empty root; every other entry has an already-declared
directory parent. Names are unique ignoring ASCII case. Headers/requests are bounded at 4 KiB,
aggregate tree metadata at 8 MiB, entry count at 65,536 and the complete transfer at 1 TiB. Stream
buffers are bounded at 64 KiB. Both received and normalized encodings must fit these limits.
Incomplete framing, oversized encodings or failed hashes never replace a retained tree.

`native_v2_files::TargetFileHost` supplies list, replacement, download, removal, private pinning,
run-only staging, materialization, managed publication, release and reclamation. Hosts inject
existing authenticated scope/member authority with no fallback to a caller filesystem. Custom
servers install it explicitly through `TargetFilesMount`; existing host implementations need no
changes. Worker artifact receipts and workspace recovery storage retain their existing contracts.
