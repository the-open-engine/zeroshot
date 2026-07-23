//! CLI to drive + measure the workspace data-plane core.
//!   publish     --tree <dir> --store <dir> [--state <index.json>] [--parent <digest>]
//!   materialize --store <dir> --manifest <digest> --out <dir>
//!   bench       --tree <dir> --store <dir>            # publish then cold-materialize, timed

use anyhow::{Context, Result};
use capsule_workspace_core::cas::{ChunkIndex, LocalBlobStore};
use capsule_workspace_core::daemon;
use capsule_workspace_core::manifest::Manifest;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Capsule workspace data-plane core (prototype)")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    Publish {
        #[arg(long)]
        tree: PathBuf,
        #[arg(long)]
        store: PathBuf,
        #[arg(long)]
        state: Option<PathBuf>,
        #[arg(long)]
        parent: Option<String>,
        /// 0 = single-threaded streaming; >0 = bounded-parallel pipeline with N compressors (E6)
        #[arg(long, default_value_t = 0)]
        workers: usize,
        /// O10: node-local stat cache enabling the "don't re-hash a quiescent file" skip. Chains
        /// generations by itself (it records the manifest it was taken against). Requires `--workers > 0`
        /// (the skip lives in the pipelined publish). Omit = re-hash the whole tree, as before.
        #[arg(long)]
        stat_cache: Option<PathBuf>,
    },
    Materialize {
        #[arg(long)]
        store: PathBuf,
        #[arg(long)]
        manifest: String,
        #[arg(long)]
        out: PathBuf,
        /// Incremental (reflink) resume: a CLEAN prior-materialization dir to reflink unchanged files
        /// from. Requires `--ref-manifest`. `out` must be empty and distinct from `--reference`.
        #[arg(long)]
        reference: Option<PathBuf>,
        /// The manifest digest that `--reference` was materialized from.
        #[arg(long)]
        ref_manifest: Option<String>,
    },
    Bench {
        #[arg(long)]
        tree: PathBuf,
        #[arg(long)]
        store: PathBuf,
    },
    /// Production daemon loop: materialize-latest-on-start → interval publish → SIGTERM final publish.
    /// Blobs live in `--store` (file:// or s3://); lineage + GC clock live in `--db` (Postgres,
    /// REQUIRED). Requires the `pg` feature (and `s3` for an s3:// store). See `daemon_loop.rs`.
    Daemon {
        /// The workspace tree to publish / materialize into.
        #[arg(long)]
        tree: PathBuf,
        /// Blob store URI: `file://<path>` (LocalBlobStore) or `s3://<bucket>` (S3BlobStore).
        #[arg(long)]
        store: String,
        /// Postgres URL for the lineage HEAD + the block_ref GC clock (REQUIRED).
        #[arg(long)]
        db: String,
        /// Lineage id this daemon owns (single-writer via the fence CAS).
        #[arg(long)]
        lineage: String,
        /// Seconds between publish cycles.
        #[arg(long, default_value_t = 30)]
        publish_interval: u64,
        /// Optional `ip:port` for the raw-TCP `/health` readiness endpoint (503 until ready, 200 after).
        #[arg(long)]
        health_addr: Option<String>,
        /// Optional node-local cache dir (NVMe): wraps the durable store so warm reads are served
        /// locally instead of over the network — fast warm resume.
        #[arg(long)]
        cache_dir: Option<String>,
        /// Run exactly ONE publish cycle and exit (for tests / one-shot snapshots).
        #[arg(long)]
        once: bool,
        /// Publish-pipeline width (compressor + uploader threads). 0 = auto (available parallelism,
        /// capped at 16). Raise it on a big node to overlap more S3 PUTs (publish is upload-bound).
        #[arg(long, default_value_t = 0)]
        publish_workers: usize,
        /// Optional pristine-reference root for WARM RESUME (O7). When set, materialize-on-start keeps an
        /// immutable reference materialization here (scoped per lineage) and reflinks unchanged files into
        /// `--tree`, fetching only the delta. MUST be on the same filesystem as `--tree` — cross-fs falls
        /// back to a full copy, which is SLOWER than omitting the flag. MUST be EPHEMERAL node-local storage
        /// (same class as `--cache-dir`): the reflink clone does not re-hash, so it trusts the reference,
        /// which is safe only because a node power-event wipes ephemeral storage → cold resume. Single-writer
        /// per lineage. Omit = full cold materialize (unchanged default).
        #[arg(long)]
        ref_dir: Option<PathBuf>,
        /// Optional node-local stat cache (O10): lets a publish cycle reuse the parent manifest's chunk
        /// list for files it can prove are quiescent, instead of re-reading + re-sha256ing the whole tree
        /// every interval. Measured on a 1 GB tree: idle cycle 1.81s → 0.01s. Node-local + ephemeral, like
        /// `--cache-dir`; it fails safe (absent/corrupt ⇒ full re-hash). Omit = re-hash every cycle.
        #[arg(long)]
        stat_cache: Option<PathBuf>,
    },
    /// Store-wide GC sweep (feature `pg`): reclaim aged, unreferenced blocks. Marks against EVERY
    /// lineage's HEAD (store-wide — a per-lineage sweep would delete other lineages' live blocks).
    /// This is a SINGLETON actor, run separately from the per-lineage daemons.
    Gc {
        /// Blob store URI: `file://<path>` or `s3://<bucket>` (the durable authority — no cache).
        #[arg(long)]
        store: String,
        /// Postgres URL (the lineage HEADs + the block_ref clock).
        #[arg(long)]
        db: String,
        /// Grace period: a block is collectable only if unreferenced AND older than this. MUST exceed
        /// max(publish duration, sweep duration) + clock skew.
        #[arg(long, default_value_t = 3600)]
        grace_secs: u64,
    },
}

fn load_known(state: &Option<PathBuf>) -> Result<ChunkIndex> {
    match state {
        Some(p) if p.exists() => {
            let bytes =
                std::fs::read(p).with_context(|| format!("reading dedup state {}", p.display()))?;
            // FAIL FAST > silent: a corrupt/partial state file must NOT be treated as empty (that
            // silently re-uploads everything and masks the corruption). Surface it.
            serde_json::from_slice(&bytes)
                .with_context(|| format!("parsing dedup state {} (corrupt?)", p.display()))
        }
        _ => Ok(ChunkIndex::new()),
    }
}

fn save_known_from_manifest(
    store: &LocalBlobStore,
    digest: &str,
    state: &Option<PathBuf>,
) -> Result<()> {
    if let Some(p) = state {
        use capsule_workspace_core::cas::BlobStore;
        let m = Manifest::from_bytes(&store.get_manifest(digest)?)?;
        std::fs::write(p, serde_json::to_vec(&m.chunks)?)?;
    }
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Publish {
            tree,
            store,
            state,
            parent,
            workers,
            stat_cache,
        } => {
            use capsule_workspace_core::cas::BlobStore;
            use capsule_workspace_core::stat_cache::StatCache;
            let s = LocalBlobStore::new(&store)?;
            let known = load_known(&state)?;
            // The cache names the manifest it was taken against, so it chains generations on its own.
            // `PrevPublish::new` refuses any pair whose digests disagree (⇒ full re-hash).
            let cache = stat_cache
                .as_deref()
                .map(StatCache::load)
                .unwrap_or_default();
            let parent_manifest = (!cache.manifest_digest.is_empty())
                .then(|| s.get_manifest(&cache.manifest_digest).ok())
                .flatten()
                .and_then(|b| Manifest::from_bytes(&b).ok());
            let prev = parent_manifest
                .as_ref()
                .and_then(|m| daemon::PrevPublish::new(m, &cache.manifest_digest, &cache));
            let stats = if workers == 0 {
                daemon::publish(&tree, &s, &known, parent)?
            } else {
                daemon::publish_pipelined(&tree, &s, &known, parent, workers, 8, prev)?
            };
            println!("{}", serde_json::to_string_pretty(&stats)?);
            save_known_from_manifest(&s, &stats.manifest, &state)?;
            if let Some(p) = stat_cache.as_deref() {
                stats.stat_cache.save(p)?;
            }
        }
        Cmd::Materialize {
            store,
            manifest,
            out,
            reference,
            ref_manifest,
        } => {
            let s = LocalBlobStore::new(&store)?;
            let stats = match (reference, ref_manifest) {
                (Some(rdir), Some(rman)) => {
                    daemon::materialize_incremental(&s, &manifest, &out, &rman, &rdir)?
                }
                (None, None) => daemon::materialize(&s, &manifest, &out)?,
                _ => anyhow::bail!("--reference and --ref-manifest must be given together"),
            };
            println!("{}", serde_json::to_string_pretty(&stats)?);
        }
        Cmd::Bench { tree, store } => {
            let s = LocalBlobStore::new(&store)?;
            let p = daemon::publish(&tree, &s, &ChunkIndex::new(), None)?;
            eprintln!("[publish] {}", serde_json::to_string(&p)?);
            let out = std::env::temp_dir().join(format!("capfs_mat_{}", std::process::id()));
            let m = daemon::materialize(&s, &p.manifest, &out)?;
            eprintln!("[materialize] {}", serde_json::to_string(&m)?);
            let _ = std::fs::remove_dir_all(&out);
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({"publish":p,"materialize":m}))?
            );
        }
        Cmd::Daemon {
            tree,
            store,
            db,
            lineage,
            publish_interval,
            health_addr,
            cache_dir,
            once,
            publish_workers,
            ref_dir,
            stat_cache,
        } => {
            #[cfg(feature = "pg")]
            {
                daemon_cmd::run(
                    tree,
                    store,
                    db,
                    lineage,
                    publish_interval,
                    health_addr,
                    cache_dir,
                    once,
                    publish_workers,
                    ref_dir,
                    stat_cache,
                )?;
            }
            #[cfg(not(feature = "pg"))]
            {
                // Consume the bindings so the default build doesn't warn, then fail with a fix.
                let _ = (
                    &tree,
                    &store,
                    &db,
                    &lineage,
                    publish_interval,
                    &health_addr,
                    &cache_dir,
                    once,
                    publish_workers,
                    &ref_dir,
                    &stat_cache,
                );
                anyhow::bail!(
                    "the `daemon` subcommand requires the `pg` feature (and `s3` for an s3:// \
                     store). Rebuild with: cargo build --release --features pg,s3"
                );
            }
        }
        Cmd::Gc {
            store,
            db,
            grace_secs,
        } => {
            #[cfg(feature = "pg")]
            {
                daemon_cmd::run_gc(store, db, grace_secs)?;
            }
            #[cfg(not(feature = "pg"))]
            {
                let _ = (&store, &db, grace_secs);
                anyhow::bail!(
                    "the `gc` subcommand requires the `pg` feature. Rebuild with --features pg,s3"
                );
            }
        }
    }
    Ok(())
}

/// The `daemon` subcommand's lifecycle (feature `pg`). Kept together here; the load-bearing publish
/// cycle + materialize-on-start themselves live in the library (`daemon_loop`) so the tests can drive
/// them directly. THREADING (MF2): a plain sync `fn main` — signals, the health server, and the
/// publish/materialize pipeline all run on std threads with NO ambient tokio runtime, so the S3/PG
/// adapters' internal `block_on`s never nest-panic.
#[cfg(feature = "pg")]
mod daemon_cmd {
    use anyhow::{Context, Result};
    use capsule_workspace_core::cas::{BlobStore, LocalBlobStore};
    use capsule_workspace_core::daemon_loop::{self, CycleOutcome};
    use capsule_workspace_core::ifaces::LineageId;
    use capsule_workspace_core::pg::{PgLineageStore, PgRefClock};
    use std::net::TcpListener;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    /// Dispatch `--store <uri>` to a concrete `BlobStore`: `file://<path>` → LocalBlobStore,
    /// `s3://<bucket>` → S3BlobStore (feature `s3`; endpoint/region/creds via the env contract).
    /// When `cache_dir` is set, wrap the durable store in a node-local `CachedBlobStore` (NVMe cache)
    /// so a warm node serves blocks locally instead of over the network — fast warm resume.
    fn build_store(uri: &str, cache_dir: Option<&str>) -> Result<Box<dyn BlobStore>> {
        let backing: Box<dyn BlobStore> = if let Some(path) = uri.strip_prefix("file://") {
            Box::new(LocalBlobStore::new(path)?)
        } else if let Some(bucket) = uri.strip_prefix("s3://") {
            #[cfg(feature = "s3")]
            {
                // Bucket comes from the URI (the CLI contract); endpoint (MinIO/localstack) from env.
                let endpoint = std::env::var("S3_ENDPOINT_URL")
                    .ok()
                    .filter(|s| !s.is_empty());
                Box::new(capsule_workspace_core::s3::S3BlobStore::new(
                    bucket, endpoint,
                )?)
            }
            #[cfg(not(feature = "s3"))]
            {
                let _ = bucket;
                anyhow::bail!(
                    "an s3:// store requires the `s3` feature — rebuild with --features pg,s3"
                )
            }
        } else {
            anyhow::bail!("unrecognized --store {uri:?}: expected file://<path> or s3://<bucket>")
        };
        match cache_dir {
            Some(dir) => Ok(Box::new(
                capsule_workspace_core::cache::CachedBlobStore::new(dir, backing)?,
            )),
            None => Ok(backing),
        }
    }

    /// Sleep up to `secs`, waking early if `shutdown` is set — so SIGTERM/SIGINT is felt within
    /// ~200 ms even mid-interval.
    fn interruptible_sleep(secs: u64, shutdown: &AtomicBool) {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < deadline {
            if shutdown.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// One publish cycle + a human log line; returns the outcome so the loop can back off on a
    /// genuine fence. A non-fence error propagates and crashes the daemon (fail-fast); a lost fence
    /// race is logged PROMINENTLY and deferred.
    fn run_and_log_cycle(
        tree: &Path,
        store: &dyn BlobStore,
        ls: &PgLineageStore,
        clock: &PgRefClock,
        lineage: &LineageId,
        publish_workers: usize,
        stat_cache: Option<&Path>,
    ) -> Result<CycleOutcome> {
        let outcome = daemon_loop::publish_cycle(
            tree,
            store,
            ls,
            clock,
            lineage,
            publish_workers,
            stat_cache,
        )?;
        match &outcome {
            CycleOutcome::Advanced(h) => eprintln!(
                "[daemon] published: lineage={} fence={} manifest={}",
                lineage.0, h.fence.0, h.manifest_digest
            ),
            CycleOutcome::NoChange => {
                eprintln!("[daemon] no change (tree unchanged) — skipped commit")
            }
            CycleOutcome::Fenced { expected, current } => eprintln!(
                "[daemon] WARNING: lineage {} fenced — another writer owns it; deferring \
                 (expected fence {}, current {})",
                lineage.0, expected.0, current.0
            ),
        }
        Ok(outcome)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn run(
        tree: PathBuf,
        store_uri: String,
        db: String,
        lineage: String,
        publish_interval: u64,
        health_addr: Option<String>,
        cache_dir: Option<String>,
        once: bool,
        publish_workers: usize,
        ref_dir: Option<PathBuf>,
        stat_cache: Option<PathBuf>,
    ) -> Result<()> {
        // 0. `--ref-dir` must not be nested with `--tree`: the reference lives inside `tree` (or vice
        //    versa) would corrupt the empty-workspace check + the ref GC. Reject up front (lexical guard).
        if let Some(rd) = ref_dir.as_deref() {
            if rd.starts_with(&tree) || tree.starts_with(rd) {
                anyhow::bail!(
                    "--ref-dir ({}) and --tree ({}) must not be nested",
                    rd.display(),
                    tree.display()
                );
            }
        }
        // 1. Postgres = lineage + GC clock (REQUIRED). A bad URL fails fast here.
        let ls = PgLineageStore::connect(&db).context("connect --db Postgres")?;
        ls.init_schema().context("apply bootstrap schema")?;
        let clock = ls.ref_clock();
        let store = build_store(&store_uri, cache_dir.as_deref()).context("build --store")?;
        let lineage = LineageId(lineage);

        // Readiness (materialize-on-start done) + shutdown (SIGTERM/SIGINT) latches — std only (MF2).
        let ready = Arc::new(AtomicBool::new(false));
        let shutdown = Arc::new(AtomicBool::new(false));
        signal_hook::flag::register(signal_hook::consts::SIGTERM, Arc::clone(&shutdown))
            .context("register SIGTERM")?;
        signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&shutdown))
            .context("register SIGINT")?;

        // Optional health/readiness endpoint (std TcpListener on its own thread — no async runtime).
        if let Some(addr) = health_addr.as_deref() {
            let listener =
                TcpListener::bind(addr).with_context(|| format!("bind --health-addr {addr}"))?;
            daemon_loop::spawn_health_server(listener, Arc::clone(&ready));
            eprintln!("[daemon] health/readiness on {addr} (503 until ready)");
        }

        // 2. Materialize-latest-on-start (fallible head read), THEN arm readiness. With `--ref-dir` this
        //    is a reflink warm resume (fetch only the delta); without it, a full cold materialize.
        let restored = daemon_loop::materialize_on_start(
            store.as_ref(),
            &ls,
            &lineage,
            &tree,
            ref_dir.as_deref(),
        )
        .context("materialize-on-start")?;
        ready.store(true, Ordering::SeqCst);
        eprintln!(
            "[daemon] ready — materialize-on-start: {} (lineage={}, tree={})",
            if restored {
                "restored HEAD"
            } else {
                "no HEAD (empty start)"
            },
            lineage.0,
            tree.display()
        );

        // 3. One cycle (--once) or the interval loop.
        if once {
            run_and_log_cycle(
                &tree,
                store.as_ref(),
                &ls,
                &clock,
                &lineage,
                publish_workers,
                stat_cache.as_deref(),
            )?;
            return Ok(());
        }
        eprintln!(
            "[daemon] publish loop every {publish_interval}s (SIGTERM/SIGINT → one final publish, \
             exit 0)"
        );
        // Back off on a genuine fence streak (reviewer S2): a fenced daemon does the expensive publish
        // (upload delta + manifest) BEFORE the cheap CAS, so re-contending every interval leaks S3
        // cost + orphan manifests and can oscillate HEAD with the other writer. Exponential backoff
        // (capped) throttles that; a successful advance resets it. (In prod the orchestrator
        // fences-by-kill; the standalone daemon has no such guard, hence the backoff.)
        let mut fenced_streak: u32 = 0;
        loop {
            let sleep_secs = if fenced_streak == 0 {
                publish_interval
            } else {
                publish_interval
                    .saturating_mul(1u64 << fenced_streak.min(5))
                    .min(600)
            };
            interruptible_sleep(sleep_secs, &shutdown);
            if shutdown.load(Ordering::SeqCst) {
                break;
            }
            match run_and_log_cycle(
                &tree,
                store.as_ref(),
                &ls,
                &clock,
                &lineage,
                publish_workers,
                stat_cache.as_deref(),
            )? {
                CycleOutcome::Fenced { .. } => fenced_streak = (fenced_streak + 1).min(6),
                CycleOutcome::Advanced(_) | CycleOutcome::NoChange => fenced_streak = 0,
            }
        }

        // 4. Drain: one final publish, then exit 0 (well within the ~30s budget).
        eprintln!("[daemon] shutdown signal — final publish (drain)");
        run_and_log_cycle(
            &tree,
            store.as_ref(),
            &ls,
            &clock,
            &lineage,
            publish_workers,
            stat_cache.as_deref(),
        )?;
        eprintln!("[daemon] drained; exiting 0");
        Ok(())
    }

    /// Store-wide GC sweep: mark against EVERY lineage's HEAD (F2), then reclaim aged orphans. Uses
    /// the raw durable store (no cache — GC deletes from the authority). Prints `GcPgStats` as JSON.
    pub fn run_gc(store_uri: String, db: String, grace_secs: u64) -> Result<()> {
        use capsule_workspace_core::gc_pg;
        let ls = PgLineageStore::connect(&db).context("connect --db Postgres")?;
        ls.init_schema().context("apply bootstrap schema")?;
        let clock = ls.ref_clock();
        let store = build_store(&store_uri, None).context("build --store")?;
        // F2: the STORE-WIDE live set — every lineage's HEAD.
        let live = ls.all_head_digests().context("read all live HEADs")?;
        let st = gc_pg::collect(
            store.as_ref(),
            &clock,
            &live,
            Duration::from_secs(grace_secs),
        )?;
        eprintln!(
            "[gc] live_heads={} grace={}s → scanned={} deleted={} kept_marked={} raced_young={} orphaned={} missing_live={}",
            live.len(),
            grace_secs,
            st.scanned,
            st.deleted,
            st.kept_marked,
            st.raced_young,
            st.orphaned,
            st.missing_live_manifests
        );
        println!("{}", serde_json::to_string(&st)?);
        Ok(())
    }
}
