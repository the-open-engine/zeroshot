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
    },
    Materialize {
        #[arg(long)]
        store: PathBuf,
        #[arg(long)]
        manifest: String,
        #[arg(long)]
        out: PathBuf,
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
        /// Run exactly ONE publish cycle and exit (for tests / one-shot snapshots).
        #[arg(long)]
        once: bool,
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
        } => {
            let s = LocalBlobStore::new(&store)?;
            let known = load_known(&state)?;
            let stats = if workers == 0 {
                daemon::publish(&tree, &s, &known, parent)?
            } else {
                daemon::publish_pipelined(&tree, &s, &known, parent, workers, 8)?
            };
            println!("{}", serde_json::to_string_pretty(&stats)?);
            save_known_from_manifest(&s, &stats.manifest, &state)?;
        }
        Cmd::Materialize {
            store,
            manifest,
            out,
        } => {
            let s = LocalBlobStore::new(&store)?;
            let stats = daemon::materialize(&s, &manifest, &out)?;
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
            once,
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
                    once,
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
                    once,
                );
                anyhow::bail!(
                    "the `daemon` subcommand requires the `pg` feature (and `s3` for an s3:// \
                     store). Rebuild with: cargo build --release --features pg,s3"
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
    fn build_store(uri: &str) -> Result<Box<dyn BlobStore>> {
        if let Some(path) = uri.strip_prefix("file://") {
            Ok(Box::new(LocalBlobStore::new(path)?))
        } else if let Some(bucket) = uri.strip_prefix("s3://") {
            #[cfg(feature = "s3")]
            {
                // Bucket comes from the URI (the CLI contract); endpoint (MinIO/localstack) from env.
                let endpoint = std::env::var("S3_ENDPOINT_URL")
                    .ok()
                    .filter(|s| !s.is_empty());
                Ok(Box::new(capsule_workspace_core::s3::S3BlobStore::new(
                    bucket, endpoint,
                )?))
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

    /// One publish cycle + a human log line. A genuine (non-fence) error propagates and crashes the
    /// daemon (fail-fast); a lost fence race is logged PROMINENTLY and deferred to the next cycle.
    fn run_and_log_cycle(
        tree: &Path,
        store: &dyn BlobStore,
        ls: &PgLineageStore,
        clock: &PgRefClock,
        lineage: &LineageId,
    ) -> Result<()> {
        match daemon_loop::publish_cycle(tree, store, ls, clock, lineage)? {
            CycleOutcome::Advanced(h) => eprintln!(
                "[daemon] published: lineage={} fence={} manifest={}",
                lineage.0, h.fence.0, h.manifest_digest
            ),
            CycleOutcome::Fenced { expected, current } => eprintln!(
                "[daemon] WARNING: lineage {} fenced — another writer owns it; deferring \
                 (expected fence {}, current {})",
                lineage.0, expected.0, current.0
            ),
        }
        Ok(())
    }

    pub fn run(
        tree: PathBuf,
        store_uri: String,
        db: String,
        lineage: String,
        publish_interval: u64,
        health_addr: Option<String>,
        once: bool,
    ) -> Result<()> {
        // 1. Postgres = lineage + GC clock (REQUIRED). A bad URL fails fast here.
        let ls = PgLineageStore::connect(&db).context("connect --db Postgres")?;
        ls.init_schema().context("apply bootstrap schema")?;
        let clock = ls.ref_clock();
        let store = build_store(&store_uri).context("build --store")?;
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

        // 2. Materialize-latest-on-start (fallible head read), THEN arm readiness.
        let restored = daemon_loop::materialize_on_start(store.as_ref(), &ls, &lineage, &tree)
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
            return run_and_log_cycle(&tree, store.as_ref(), &ls, &clock, &lineage);
        }
        eprintln!(
            "[daemon] publish loop every {publish_interval}s (SIGTERM/SIGINT → one final publish, \
             exit 0)"
        );
        loop {
            interruptible_sleep(publish_interval, &shutdown);
            if shutdown.load(Ordering::SeqCst) {
                break;
            }
            run_and_log_cycle(&tree, store.as_ref(), &ls, &clock, &lineage)?;
        }

        // 4. Drain: one final publish, then exit 0 (well within the ~30s budget).
        eprintln!("[daemon] shutdown signal — final publish (drain)");
        run_and_log_cycle(&tree, store.as_ref(), &ls, &clock, &lineage)?;
        eprintln!("[daemon] drained; exiting 0");
        Ok(())
    }
}
