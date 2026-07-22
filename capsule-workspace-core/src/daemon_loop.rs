//! Daemon run-loop helpers — the small, library-level pieces the `daemon` subcommand drives and the
//! integration tests exercise directly (a binary's `fn`s are not reachable from `tests/`, so the
//! load-bearing logic lives here in the lib):
//!   - `health_response` / `spawn_health_server`: the minimal std-only readiness/liveness endpoint.
//!   - `materialize_on_start` + `publish_cycle` (feature `pg`): materialize-latest-on-start and one
//!     publish cycle in the load-bearing MF3 (dedup `known` = parent live-HEAD chunk index) + MF4
//!     (touch every referenced block young BEFORE advancing HEAD) ordering.
//!
//! THREADING (MF2 — the one way the sync/`block_on` adapters would panic): everything here runs on
//! plain std threads with NO ambient tokio runtime. The S3/PG adapters each own an internal
//! multi-thread runtime and `block_on` inside their sync trait methods; if the daemon were
//! `#[tokio::main]` (or otherwise ran these under a runtime) those `block_on`s would nest-panic
//! ("cannot start a runtime from within a runtime"). So the publish/materialize pipeline, the signal
//! latch, and this health server are all std/thread-based on purpose.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

/// The raw HTTP/1.1 response the health endpoint returns. Readiness (= materialize-on-start
/// completed, a shared `AtomicBool`) gates it: ready → `200 ok`, not-yet-ready → `503`. Liveness is
/// implied by the process answering at all. Two-byte bodies keep `content-length` trivial.
pub fn health_response(ready: bool) -> &'static str {
    if ready {
        "HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok"
    } else {
        "HTTP/1.1 503 Service Unavailable\r\ncontent-length: 2\r\n\r\nno"
    }
}

fn serve_health(mut stream: TcpStream, ready: bool) {
    // Bound the read/write so a half-open or slow client can't wedge the single-threaded accept loop
    // (which would block ALL subsequent probes → readiness/liveness stuck → the kubelet kills the pod)
    // (reviewer S1). Best-effort drain of the request so the client's write doesn't RST before we
    // reply; we don't parse it (any path is a probe). Bounded read — never block on a huge body.
    let to = Some(std::time::Duration::from_secs(2));
    let _ = stream.set_read_timeout(to);
    let _ = stream.set_write_timeout(to);
    let mut scratch = [0u8; 1024];
    let _ = stream.read(&mut scratch);
    let _ = stream.write_all(health_response(ready).as_bytes());
    let _ = stream.flush();
}

/// Spawn the minimal health server on a std thread over an already-bound `listener` (so callers — the
/// daemon and the tests — control binding, incl. `127.0.0.1:0` for an ephemeral test port). Each
/// connection is answered from the CURRENT value of `ready`: 503 before readiness, 200 after. Runs
/// until the process exits (a detached daemon thread once the returned handle is dropped); no async
/// runtime involved (MF2).
pub fn spawn_health_server(listener: TcpListener, ready: Arc<AtomicBool>) -> JoinHandle<()> {
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            match conn {
                Ok(stream) => serve_health(stream, ready.load(Ordering::SeqCst)),
                Err(_) => continue, // transient accept error — keep serving
            }
        }
    })
}

#[cfg(feature = "pg")]
mod cycle {
    use crate::cas::{BlobStore, BlockId, ChunkIndex};
    use crate::daemon::{materialize, publish_pipelined, resume_via_reference};
    use crate::ifaces::{Fence, LineageId};
    use crate::lineage::{Head, LineageError, LineageStore};
    use crate::manifest::Manifest;
    use crate::pg::PgLineageStore;
    use crate::refclock::RefClock;
    use anyhow::Result;
    use std::path::Path;

    /// The outcome of one publish cycle. `Fenced` is the NON-FATAL "another writer owns this lineage"
    /// case (a genuine StaleFence, AFTER `advance`'s idempotent-retry already absorbed our own
    /// lost-acks): the daemon logs it and defers to the next cycle, which re-reads the new HEAD. It is
    /// surfaced as a value (not an `Err`) so the loop never crashes on a lost fence race.
    #[derive(Debug)]
    pub enum CycleOutcome {
        /// HEAD advanced to (or, idempotently, already held) this fence.
        Advanced(Head),
        /// The tree is byte-identical to the live HEAD (content-only digest unchanged) → nothing to
        /// commit. We skip touch + advance entirely: the blocks are already MARK-protected via the
        /// unchanged HEAD, so an idle daemon does ZERO writes (no fence bump, no manifest churn — F1).
        NoChange,
        /// The lineage is fenced by a DIFFERENT writer — defer, don't crash.
        Fenced { expected: Fence, current: Fence },
    }

    /// Materialize the current HEAD into `tree` on daemon start, using the FALLIBLE `head()` read (NOT
    /// the infallible trait `get`): a transient DB error is a hard error (don't start a pod with an
    /// empty workspace on a blip), and "no HEAD yet" is a clean no-op. Returns whether a HEAD existed
    /// (i.e. materialization ran). `tree` must be empty/absent — `materialize` enforces that.
    pub fn materialize_on_start(
        store: &dyn BlobStore,
        ls: &PgLineageStore,
        lineage: &LineageId,
        tree: &Path,
        ref_dir: Option<&Path>,
    ) -> Result<bool> {
        match ls.head(lineage)? {
            Some(h) => {
                match ref_dir {
                    // Warm resume: reflink unchanged files from a daemon-owned pristine reference, fetch
                    // only the delta (O7). `ref_dir` should be on the same filesystem as `tree`. Scoped by
                    // lineage so multiple lineages can share one `--ref-dir` without clobbering each other.
                    Some(rroot) => {
                        let scoped = rroot.join(crate::daemon::lineage_ref_subdir(&lineage.0));
                        let st = resume_via_reference(store, &h.manifest_digest, tree, &scoped)?;
                        eprintln!(
                            "[daemon] warm resume ({:?}): ref_blocks_fetched={} ref_reflinked={} \
                             workspace_files={}",
                            st.kind, st.ref_blocks_fetched, st.ref_reflinked, st.workspace_files
                        );
                    }
                    // Default (no --ref-dir): full cold materialize straight into the workspace.
                    None => {
                        materialize(store, &h.manifest_digest, tree)?;
                    }
                }
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Run ONE publish cycle in the load-bearing MF3+MF4 ordering:
    ///
    /// 1. `ls.head()` — the FALLIBLE authoritative read (a DB blip is an `Err`, not "fresh lineage").
    /// 2. Seed dedup `known` from the PARENT LIVE-HEAD's chunk index (MF3): every dedup-reused block
    ///    then comes from a manifest GC MARKS (the parent HEAD is within retention), so a reused block
    ///    can never be collected out from under us. `parent`/`expected` come from the SAME HEAD.
    /// 3. `publish()` — uploads new blocks + puts the new manifest (idempotent, content-addressed).
    /// 4. MF4 touch: refresh the reuse-clock for EVERY block the new manifest references, AFTER upload
    ///    but BEFORE advancing HEAD. (Block ids aren't known until packed, so pre-upload touch is
    ///    impossible for a streaming publish; post-upload is the correct realization of the invariant.)
    ///    Safe because a freshly-uploaded-but-untouched block has NO `block_ref` row and is invisible
    ///    to GC's candidate scan; once touched it is young; once the HEAD advances it is MARK-protected.
    ///    Continuous cover: no-row (invisible) → young-row (grace) → marked (reachable).
    /// 5. `advance()` fence CAS. Its built-in idempotent-retry turns a lost-ack of OUR OWN write into
    ///    `Ok`, so a StaleFence reaching us here is a DIFFERENT writer → surface `Fenced` (non-fatal).
    pub fn publish_cycle(
        tree: &Path,
        store: &dyn BlobStore,
        ls: &PgLineageStore,
        clock: &dyn RefClock,
        lineage: &LineageId,
        req_workers: usize,
    ) -> Result<CycleOutcome> {
        let head = ls.head(lineage)?; // fallible authoritative read
        let (known, parent, expected): (ChunkIndex, Option<String>, Fence) = match &head {
            Some(h) => {
                // MF3: dedup set = the parent live-HEAD's chunk index (a manifest GC marks).
                let m = Manifest::from_bytes(&store.get_manifest(&h.manifest_digest)?)?;
                (m.chunks, Some(h.manifest_digest.clone()), h.fence)
            }
            None => (ChunkIndex::new(), None, Fence(0)),
        };

        // O3: the daemon uses the bounded-PARALLEL publish pipeline (measured ~2× faster than the
        // single-threaded streaming publish on a 2 GB tree: 20.4s→10.4s at 16 workers, plateauing
        // there). `req_workers == 0` → auto = available parallelism capped at 16 (a 2–4 vCPU pod uses
        // 2–4; a big node gains nothing past 16 for CPU, though real-S3 publish is upload-bound so more
        // uploader threads still overlap PUTs). A non-zero value is an explicit operator override
        // (`--publish-workers`), clamped to a sane ceiling. Produces the SAME logical manifest either way.
        let workers = if req_workers == 0 {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
                .clamp(1, 16)
        } else {
            req_workers.clamp(1, 64)
        };
        let stats = publish_pipelined(tree, store, &known, parent, workers, 8)?;

        // Change-detection: with content-only manifest identity, an unchanged tree yields the SAME
        // digest as the live HEAD → nothing to commit. Skip touch + advance so an idle daemon does no
        // writes at all (no fence bump, no block_ref churn). Its blocks stay protected by the MARK on
        // the unchanged HEAD. (`publish` still walked/hashed the tree — an mtime-skip to avoid even
        // that is a later throughput optimization.)
        if let Some(h) = &head {
            if stats.manifest == h.manifest_digest {
                return Ok(CycleOutcome::NoChange);
            }
        }

        // MF4: touch every block the new manifest references (young) BEFORE advancing HEAD. `touch`
        // dedups by block, so accumulate the block's REAL size = sum of its member chunks' compressed
        // lengths (not one arbitrary chunk's `clen`, which would under-report byte_length ~256×).
        let m = Manifest::from_bytes(&store.get_manifest(&stats.manifest)?)?;
        let mut sizes: std::collections::BTreeMap<BlockId, u64> = std::collections::BTreeMap::new();
        for loc in m.chunks.values() {
            *sizes.entry(loc.block.clone()).or_insert(0) += loc.clen as u64;
        }
        let touch: Vec<(BlockId, u64)> = sizes.into_iter().collect();
        clock.touch(&touch)?;

        match ls.advance(lineage, stats.manifest.clone(), expected) {
            Ok(h) => Ok(CycleOutcome::Advanced(h)),
            Err(e) => match e.downcast_ref::<LineageError>() {
                Some(LineageError::StaleFence { expected, current }) => Ok(CycleOutcome::Fenced {
                    expected: *expected,
                    current: *current,
                }),
                None => Err(e), // a non-fence error is fatal (fail-fast)
            },
        }
    }
}

#[cfg(feature = "pg")]
pub use cycle::{materialize_on_start, publish_cycle, CycleOutcome};
