//! O10 — node-local stat cache backing the publish "don't re-hash an unchanged file" skip.
//!
//! `publish` reads + sha256s EVERY file EVERY cycle; only the upload is deduped. Measured on a 1 GB /
//! 256-file tree: an UNCHANGED republish costs 1.81s and a 0.4%-churn republish also costs 1.81s — the cost
//! tracks TREE SIZE, not churn, and recurs every publish interval forever. This cache lets a publish reuse
//! the PARENT MANIFEST's chunk list for files that are provably quiescent, so an idle cycle becomes
//! ~stat-only.
//!
//! THE HARD CONSTRAINT: a wrongly-skipped file means the published manifest keeps the file's OLD chunk
//! list, so the agent's actual work is silently NOT durable and a later resume reverts it. That is the worst
//! failure mode in this system — worse than a crash, which is at least loud. So every rule here is
//! conservative: **any doubt ⇒ re-hash**. Being slow is free; being wrong is unrecoverable.
//!
//! Deliberately NOT part of the manifest: keeping stat data out of `Manifest` preserves `logical_digest` as
//! pure CONTENT identity and the O1 idle-republish idempotence (a manifest must not change just because an
//! mtime did).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Bump when the fingerprint's meaning changes — an older cache is then ignored (⇒ full re-hash) rather
/// than misinterpreted.
pub const STAT_CACHE_VERSION: u32 = 1;

/// A file's stat fingerprint. Equality alone is NOT sufficient to skip (see [`StatCache::may_skip`]).
///
/// Field rationale — each closes a distinct way a change could hide:
/// - `size`     — catches almost all content change for free.
/// - `mtime_ns` — the primary change signal.
/// - `ctime_ns` — catches mtime BACKDATING (`utimes` rewrites mtime but ctime becomes now, and a non-root
///                process cannot forge ctime), plus chmod/rename/link changes.
/// - `ino`      — catches replace-by-rename, which can otherwise preserve size and mtime exactly.
/// - `dev`      — catches a remount / different device under the same path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatKey {
    pub size: u64,
    pub mtime_ns: i64,
    pub ctime_ns: i64,
    pub ino: u64,
    pub dev: u64,
}

impl StatKey {
    pub fn from_metadata(m: &std::fs::Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;
        // saturating: a pathological clock can't panic a publish.
        let ns = |s: i64, n: i64| s.saturating_mul(1_000_000_000).saturating_add(n);
        Self {
            size: m.len(),
            mtime_ns: ns(m.mtime(), m.mtime_nsec()),
            ctime_ns: ns(m.ctime(), m.ctime_nsec()),
            ino: m.ino(),
            dev: m.dev(),
        }
    }
}

/// Fingerprints from the previous publish, plus the instant that publish's walk began.
#[derive(Debug, Serialize, Deserialize)]
pub struct StatCache {
    pub version: u32,
    /// Wall-clock (unix ns) captured BEFORE the walk that produced these entries. The racy-window guard
    /// compares file mtimes against this.
    pub scan_started_ns: i64,
    pub entries: HashMap<String, StatKey>,
}

impl Default for StatCache {
    fn default() -> Self {
        Self {
            version: STAT_CACHE_VERSION,
            // 0 ⇒ every file fails the racy-window guard ⇒ nothing is ever skipped. The safe identity.
            scan_started_ns: 0,
            entries: HashMap::new(),
        }
    }
}

/// Unix nanoseconds now (0 if the clock is before the epoch — which then disables skipping, fail-safe).
pub fn now_unix_ns() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

impl StatCache {
    pub fn new(scan_started_ns: i64) -> Self {
        Self {
            version: STAT_CACHE_VERSION,
            scan_started_ns,
            entries: HashMap::new(),
        }
    }

    /// May `path` reuse the parent manifest's chunk list instead of being re-read and re-hashed?
    ///
    /// Requires BOTH:
    /// 1. the freshly-stat'd fingerprint is EXACTLY equal to the cached one; and
    /// 2. **racy-window guard** — the file was already quiescent before the previous scan began
    ///    (`mtime_ns < scan_started_ns`). Without this, a write landing in the same coarse timestamp tick
    ///    as the previous scan would be invisible: the fingerprint would match while the bytes differ.
    ///    Files touched during or after that scan are re-hashed exactly once, then become skippable.
    ///
    /// The caller MUST additionally confirm the parent manifest holds a regular-file entry for this path
    /// and take the chunk list from THERE (never from this cache) — that is what guarantees every reused
    /// chunk belongs to a successfully-published manifest and is therefore durable and GC-protected.
    pub fn may_skip(&self, path: &str, fresh: &StatKey) -> bool {
        match self.entries.get(path) {
            Some(prev) => *prev == *fresh && fresh.mtime_ns < self.scan_started_ns,
            None => false,
        }
    }

    pub fn insert(&mut self, path: String, key: StatKey) {
        self.entries.insert(path, key);
    }

    /// Load a cache. ANY problem (absent, unreadable, corrupt, version mismatch) yields the EMPTY cache,
    /// which simply means "skip nothing, re-hash everything" — always correct, just slower.
    ///
    /// This is deliberately NOT fail-fast, unlike the `--state` dedup index: treating a corrupt dedup index
    /// as empty silently RE-UPLOADS everything and hides the corruption, whereas treating a corrupt stat
    /// cache as empty is self-correcting and costs only CPU. A present-but-unusable file is still reported
    /// on stderr so it can't rot unnoticed.
    pub fn load(path: &Path) -> Self {
        if !path.exists() {
            return Self::default();
        }
        match std::fs::read(path)
            .map_err(anyhow::Error::from)
            .and_then(|b| serde_json::from_slice::<StatCache>(&b).context("parsing stat cache"))
        {
            Ok(c) if c.version == STAT_CACHE_VERSION => c,
            Ok(c) => {
                eprintln!(
                    "[publish] stat cache version {} != {STAT_CACHE_VERSION} — ignoring (full re-hash)",
                    c.version
                );
                Self::default()
            }
            Err(e) => {
                eprintln!("[publish] stat cache unusable ({e}) — ignoring (full re-hash)");
                Self::default()
            }
        }
    }

    /// Persist atomically (write sibling tmp, then rename) so a crash can't leave a torn cache that a later
    /// publish would half-trust.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec(self)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(size: u64, mtime: i64, ctime: i64, ino: u64) -> StatKey {
        StatKey {
            size,
            mtime_ns: mtime,
            ctime_ns: ctime,
            ino,
            dev: 1,
        }
    }

    // The happy path: quiescent well before the previous scan, fingerprint identical → skippable.
    #[test]
    fn skips_only_a_quiescent_identical_file() {
        let mut c = StatCache::new(1000);
        let k = key(10, 500, 500, 7);
        c.insert("a".into(), k);
        assert!(c.may_skip("a", &k));
        assert!(!c.may_skip("missing", &k), "unknown path is never skipped");
    }

    // Each fingerprint field must independently defeat the skip.
    #[test]
    fn every_fingerprint_field_defeats_the_skip() {
        let mut c = StatCache::new(1000);
        let base = key(10, 500, 500, 7);
        c.insert("a".into(), base);
        assert!(!c.may_skip("a", &key(11, 500, 500, 7)), "size change");
        assert!(!c.may_skip("a", &key(10, 501, 500, 7)), "mtime change");
        assert!(
            !c.may_skip("a", &key(10, 500, 501, 7)),
            "ctime change (mtime backdated after a write)"
        );
        assert!(
            !c.may_skip("a", &key(10, 500, 500, 8)),
            "inode change (replace-by-rename)"
        );
        let mut other_dev = base;
        other_dev.dev = 2;
        assert!(!c.may_skip("a", &other_dev), "device change");
    }

    // THE data-loss guard: a file written in the same timestamp tick as the previous scan has an
    // identical fingerprint but different bytes. The racy window must refuse to skip it.
    #[test]
    fn racy_window_refuses_files_touched_during_or_after_the_previous_scan() {
        let scan = 1000;
        let mut c = StatCache::new(scan);
        // mtime exactly AT the scan start — indistinguishable from a write during the scan.
        let at_scan = key(10, scan, scan, 7);
        c.insert("at".into(), at_scan);
        assert!(
            !c.may_skip("at", &at_scan),
            "mtime == scan start must re-hash"
        );
        // mtime AFTER the scan start.
        let after = key(10, scan + 1, scan + 1, 7);
        c.insert("after".into(), after);
        assert!(
            !c.may_skip("after", &after),
            "mtime > scan start must re-hash"
        );
        // strictly before → trustworthy.
        let before = key(10, scan - 1, scan - 1, 7);
        c.insert("before".into(), before);
        assert!(c.may_skip("before", &before));
    }

    // A default/empty cache must skip NOTHING (the safe identity), including for a zero-mtime file.
    #[test]
    fn empty_cache_skips_nothing() {
        let c = StatCache::default();
        assert!(!c.may_skip("a", &key(10, 0, 0, 7)));
        let mut c2 = StatCache::default();
        c2.insert("a".into(), key(10, 0, 0, 7));
        assert!(
            !c2.may_skip("a", &key(10, 0, 0, 7)),
            "scan_started_ns=0 ⇒ nothing is ever quiescent-before-scan"
        );
    }

    // Load must fail SAFE (empty ⇒ full re-hash) for absent, corrupt, and version-mismatched files.
    #[test]
    fn load_fails_safe() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("sc.json");
        assert!(StatCache::load(&p).entries.is_empty(), "absent → empty");

        std::fs::write(&p, b"{not json").unwrap();
        assert!(StatCache::load(&p).entries.is_empty(), "corrupt → empty");

        let mut old = StatCache::new(1000);
        old.insert("a".into(), key(10, 1, 1, 7));
        let mut v = serde_json::to_value(&old).unwrap();
        v["version"] = serde_json::json!(STAT_CACHE_VERSION + 1);
        std::fs::write(&p, serde_json::to_vec(&v).unwrap()).unwrap();
        assert!(
            StatCache::load(&p).entries.is_empty(),
            "version mismatch → empty"
        );
    }

    #[test]
    fn save_then_load_roundtrips() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("nested/sc.json");
        let mut c = StatCache::new(4242);
        c.insert("dir/f.bin".into(), key(99, 1, 2, 3));
        c.save(&p).unwrap();
        let back = StatCache::load(&p);
        assert_eq!(back.scan_started_ns, 4242);
        assert_eq!(back.entries.get("dir/f.bin"), c.entries.get("dir/f.bin"));
        assert!(
            !p.with_extension("tmp").exists(),
            "staging file renamed away"
        );
    }

    // The fingerprint must actually reflect a real file's stat, and change when the file changes.
    #[test]
    fn from_metadata_tracks_real_file_changes() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("f.bin");
        std::fs::write(&f, b"hello").unwrap();
        let k1 = StatKey::from_metadata(&std::fs::metadata(&f).unwrap());
        assert_eq!(k1.size, 5);
        assert!(k1.ino != 0 && k1.mtime_ns > 0);
        std::fs::write(&f, b"hello world").unwrap();
        let k2 = StatKey::from_metadata(&std::fs::metadata(&f).unwrap());
        assert_ne!(k1, k2, "rewriting the file changes its fingerprint");
    }
}
