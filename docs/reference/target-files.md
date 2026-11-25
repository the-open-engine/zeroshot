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
reads one. After a resume, pass the producing part ID from the receipt’s `address.scope.runId`
to `--run`. Clients cannot write or remove run publications. The host separately owns staged inputs,
private pins, request publications and final publications. References to files in a live local
workspace are not managed publications and do not appear in this tree.

An upload receipt includes its operation key. Supply `--operation` when retrying a known upload:
identical retries return the original logical result, while conflicting reuse fails. Subsequent
replacements or removals remain effective when an older operation is replayed. Pins keep the
selected bytes private and stable even after a library path is changed or deleted; hosts explicitly
release them when unused.
Managed publication names remain reserved after release so an earlier receipt cannot resolve to
another publication's bytes.

## Graph outputs and cleanup

Declare `artifacts` on a human request or graph success to expose current workspace paths. A
managed host captures bytes before the request or successful result becomes visible and before
workspace cleanup. Each selected folder keeps its name and complete layout. Publishing the same
path at a later breakpoint captures its then-current contents. Retrying one breakpoint recovers
its original committed receipt, including after a controller restart. The complete receipt must
fit the existing run-record and response limits. An oversized set fails before its managed bytes
or names are committed.

Final references appear in result `metadata.artifacts`. Request attachments appear on the
request-authorized inbox view; ordinary status and vector views omit them. Hosts apply request-read
authority to request files and run-read authority to final files. Terminal output JSON and Git
delivery receipts are independent of file publication.

Local in-place runs return `{ "kind": "local", "root": "...", "path": "..." }` references into the
recorded working folder. They read current contents, become unavailable when removed and recheck
confinement when opened. Their bytes never enter managed storage or the virtual runs tree.

Managed publications become eligible for explicit cleanup 60 days after publication. Time,
reads and startup do not remove them: until cleanup, they remain listable, downloadable and usable
as new inputs. Accepted input pins keep their bytes after a publication is removed.

```bash
zeroshot cleanup
zeroshot cleanup --target NAME
zeroshot cleanup --all
```

Cleanup on local state or a supporting direct Docker target removes eligible managed publication
sets and discardable checkpoint lineages at least 60 days after their latest terminal attempt.
`--all` removes the age filter. Active, queued, suspended and restoring work and its inputs remain
protected. Cleanup reuses resume/discard locks and reports `publications` and `checkpointLineages`
removed; repeating it reports zero. Library files, live local outputs, logs and ledgers are outside
its selection. Physical bytes are released only when their final retained reference is gone.
Discarding a failed local run's checkpoints still allows plain resume in its current checkout;
restoring a discarded snapshot is unavailable.

## Transport and host contract

The Rust protocol crate's `target_files` module owns the binary contract and constants. Its
generated `protocol/openengine-cluster/v1/target-files.schema.json` describes metadata.
The independent `extensions.files` marker has kind `openengine.target-files/v1`. An absent or unknown
marker means unsupported; existing targets keep serving their other operations.

All file HTTP operations POST to `/native-v2/files` on the selected target's authenticated origin.
They use `application/vnd.openengine.file-tree.v1`, explicit `Content-Length`, no redirects and
`Cache-Control: no-store` responses. A request starts with a big-endian u32 metadata length and
that many JSON bytes (`FileRequest`). Only `replace` appends a binary tree. List/remove/replace/cleanup
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
servers install it explicitly through `TargetFilesMount`. Run allocators inject a
`RunFilePublisher` for their placement; publication access context stays in the host catalog.
Implementations must call the supplied preflight validator with the complete receipt before
committing it, including on replay.
Existing hosts may leave publication and cleanup unsupported. Worker artifact receipts and workspace recovery storage retain their existing contracts.
