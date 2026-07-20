//! CLI to drive + measure the workspace data-plane core.
//!   publish     --tree <dir> --store <dir> [--state <index.json>] [--parent <digest>]
//!   materialize --store <dir> --manifest <digest> --out <dir>
//!   bench       --tree <dir> --store <dir>            # publish then cold-materialize, timed

use anyhow::Result;
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
}

fn load_known(state: &Option<PathBuf>) -> ChunkIndex {
    match state {
        Some(p) if p.exists() => {
            serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap_or_default()
        }
        _ => ChunkIndex::new(),
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
            let known = load_known(&state);
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
    }
    Ok(())
}
