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

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

struct VersionProbe {
    version: u32,
}

/// Bump when the fingerprint's meaning changes — an older cache is then ignored (⇒ full re-hash) rather
/// than misinterpreted.
pub const STAT_CACHE_VERSION: u32 = 2;

/// A file's stat fingerprint. Equality alone is NOT sufficient to skip (see [`StatCache::may_skip`]).
///
/// Field rationale — each closes a distinct way a change could hide:
/// - `size`     — catches almost all content change for free.
/// - `mtime_ns` — the primary change signal.
/// - `ctime_ns` — catches mtime BACKDATING (`utimes` rewrites mtime but ctime becomes now, and a non-root
///   process cannot forge ctime), plus chmod/rename/link changes.
/// - `ino`      — catches replace-by-rename, which can otherwise preserve size and mtime exactly.
/// - `dev`      — catches a remount / bind-mount / overlayfs copy-up under the same path.
/// - `nlink`    — catches hardlink churn (pnpm/cargo relinking a store entry into place).
/// - `mode`     — redundant with ctime in principle, but free and makes the fingerprint self-evident.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatKey {
    pub size: u64,
    pub mtime_ns: i64,
    pub ctime_ns: i64,
    pub ino: u64,
    pub dev: u64,
    pub nlink: u64,
    pub mode: u32,
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
            nlink: m.nlink(),
            mode: m.mode(),
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
    /// Digest of the manifest THIS scan produced. A cache may only be applied against a parent manifest
    /// with the SAME digest — see `PrevPublish::new`.
    ///
    /// Why this is load-bearing: a publish can succeed (manifest stored, cache written) and then LOSE the
    /// fence, so HEAD never advances. Without this binding, the next cycle would pair a cache describing
    /// generation N+1's tree with generation N's parent manifest, and any file changed in N+1 whose size
    /// happened to match would be "skipped" back to its STALE chunk list — silent data loss. Empty by
    /// default (and via `serde(default)` for older caches), which never matches ⇒ no skip.
    #[serde(default)]
    pub manifest_digest: String,
    pub entries: HashMap<String, StatKey>,
}

impl Default for StatCache {
    fn default() -> Self {
        Self {
            version: STAT_CACHE_VERSION,
            // 0 ⇒ every file fails the racy-window guard ⇒ nothing is ever skipped. The safe identity.
            scan_started_ns: 0,
            manifest_digest: String::new(),
            entries: HashMap::new(),
        }
    }
}

/// A file must have been quiescent for at least this long BEFORE the previous scan began to be skippable.
///
/// The racy-window guard compares a FILESYSTEM mtime against a PROCESS wall clock. On the intended
/// deployment (node-local ephemeral storage) those are the same kernel clock, but they are not the same
/// clock *domain* in general — a network filesystem's server clock, an NTP step, or a VM migration can
/// separate them. A settle margin absorbs that skew and any coarse-granularity truncation, at the cost of
/// re-hashing a file for one extra cycle after it stops changing. Publish intervals are tens of seconds, so
/// this is ~free; being wrong is not. (A filesystem-domain watermark — create a sentinel, read back its
/// mtime — is the fully general fix and is recorded as a follow-up.)
const SETTLE_NS: i64 = 2_000_000_000; // 2s

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
            manifest_digest: String::new(),
            entries: HashMap::new(),
        }
    }

    /// May `path` reuse the parent manifest's chunk list instead of being re-read and re-hashed?
    ///
    /// Requires BOTH:
    /// 1. the freshly-stat'd fingerprint is EXACTLY equal to the cached one; and
    /// 2. **racy-window guard** — BOTH `mtime_ns` and `ctime_ns` are at least `SETTLE_NS` older than the
    ///    previous scan's start. A bare `mtime < scan_started` is NOT sufficient and has been demonstrated
    ///    to lose data: filesystems truncate mtime DOWN to their granularity, which moves the comparison in
    ///    the unsafe direction, so a write landing after the scan began can still carry an mtime before it.
    ///    (Reproduced on a 1 s-granularity filesystem: same size, in-place rewrite, identical fingerprint,
    ///    file silently reverted on the next resume — and it never self-heals, because the fingerprint
    ///    never changes again.) The margin also covers the process-clock vs filesystem-clock domain gap.
    ///
    /// The caller MUST additionally confirm the parent manifest holds a regular-file entry for this path
    /// and take the chunk list from THERE (never from this cache) — that is what guarantees every reused
    /// chunk belongs to a successfully-published manifest and is therefore durable and GC-protected.
    pub fn may_skip(&self, path: &str, fresh: &StatKey) -> bool {
        match self.entries.get(path) {
            Some(prev) => {
                *prev == *fresh
                    && fresh
                        .mtime_ns
                        .saturating_add(SETTLE_NS)
                        .lt(&self.scan_started_ns)
                    && fresh
                        .ctime_ns
                        .saturating_add(SETTLE_NS)
                        .lt(&self.scan_started_ns)
            }
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
    /// The version recorded in a serialized cache, read WITHOUT attempting a full parse.
    ///
    /// Exposed so the two-stage behaviour is testable: a genuine older cache fails a full `StatCache`
    /// parse (new required fields), so a one-stage loader cannot tell "older format" from "corrupt" and
    /// reports corruption after every upgrade. A test asserting only "returns empty" passes either way.
    pub fn probe_version(bytes: &[u8]) -> Option<u32> {
        #[derive(Deserialize)]
        struct V {
            version: u32,
        }
        serde_json::from_slice::<V>(bytes).ok().map(|v| v.version)
    }

    pub fn load(path: &Path) -> Self {
        if !path.exists() {
            return Self::default();
        }
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("[publish] stat cache unreadable ({e}) — ignoring (full re-hash)");
                return Self::default();
            }
        };
        // Read the version FIRST, on its own. A full parse cannot distinguish "older format" from
        // "corrupt": adding a required field to StatKey makes every genuine older cache fail
        // deserialization, so a normal upgrade would report "corrupt" to the operator after every deploy.
        match Self::probe_version(&bytes).map(|version| VersionProbe { version }) {
            Some(v) if v.version != STAT_CACHE_VERSION => {
                eprintln!(
                    "[publish] stat cache is version {}, this build writes {STAT_CACHE_VERSION} — \
                     ignoring it and re-hashing once (expected after an upgrade)",
                    v.version
                );
                return Self::default();
            }
            None => {
                eprintln!(
                    "[publish] stat cache unusable (unreadable version) — ignoring (full re-hash)"
                );
                return Self::default();
            }
            Some(_) => {}
        }
        match serde_json::from_slice::<StatCache>(&bytes) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[publish] stat cache corrupt ({e}) — ignoring (full re-hash)");
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
        // Per-writer UNIQUE temp (same pattern as cas.rs): two processes sharing a --stat-cache path must
        // not interleave into one temp file and leave a torn rename behind.
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = path.with_extension(format!("{}.{}.tmp", std::process::id(), n));
        std::fs::write(&tmp, serde_json::to_vec(self)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Realistic nanosecond scales: the racy guard now requires a SETTLE_NS margin, so toy values like
    // mtime=500 / scan=1000 are (correctly) never skippable.
    const SCAN: i64 = 100_000_000_000; // 100s
    const OLD: i64 = 1_000_000_000; //   1s — comfortably older than SCAN - SETTLE_NS

    fn key(size: u64, mtime: i64, ctime: i64, ino: u64) -> StatKey {
        StatKey {
            size,
            mtime_ns: mtime,
            ctime_ns: ctime,
            ino,
            dev: 1,
            nlink: 1,
            mode: 0o644,
        }
    }

    // The happy path: quiescent well before the previous scan, fingerprint identical → skippable.
    #[test]
    fn skips_only_a_quiescent_identical_file() {
        let mut c = StatCache::new(SCAN);
        let k = key(10, OLD, OLD, 7);
        c.insert("a".into(), k);
        assert!(c.may_skip("a", &k));
        assert!(!c.may_skip("missing", &k), "unknown path is never skipped");
    }

    // Each fingerprint field must independently defeat the skip.
    #[test]
    fn every_fingerprint_field_defeats_the_skip() {
        let mut c = StatCache::new(SCAN);
        let base = key(10, OLD, OLD, 7);
        c.insert("a".into(), base);
        assert!(c.may_skip("a", &base), "baseline is skippable");
        assert!(!c.may_skip("a", &key(11, OLD, OLD, 7)), "size change");
        assert!(!c.may_skip("a", &key(10, OLD + 1, OLD, 7)), "mtime change");
        assert!(
            !c.may_skip("a", &key(10, OLD, OLD + 1, 7)),
            "ctime change (mtime backdated after a write)"
        );
        assert!(
            !c.may_skip("a", &key(10, OLD, OLD, 8)),
            "inode change (replace-by-rename)"
        );
        for (label, mutate) in [
            (
                "device change",
                (|k: &mut StatKey| k.dev = 2) as fn(&mut StatKey),
            ),
            ("hardlink churn (nlink)", |k: &mut StatKey| k.nlink = 2),
            ("mode change", |k: &mut StatKey| k.mode = 0o755),
        ] {
            let mut m = base;
            mutate(&mut m);
            assert!(!c.may_skip("a", &m), "{label}");
        }
    }

    // THE data-loss guard: a file written in the same timestamp tick as the previous scan has an
    // identical fingerprint but different bytes. The racy window must refuse to skip it.
    #[test]
    fn racy_window_refuses_files_touched_during_or_after_the_previous_scan() {
        let scan = SCAN;
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
        // inside the settle margin (quiescent, but only just) → still refused.
        let recent = key(10, scan - 1_000_000_000, scan - 1_000_000_000, 7);
        c.insert("recent".into(), recent);
        assert!(
            !c.may_skip("recent", &recent),
            "1s before the scan is inside the 2s settle margin → must re-hash"
        );
        // comfortably settled before the scan → trustworthy.
        let before = key(10, OLD, OLD, 7);
        c.insert("before".into(), before);
        assert!(c.may_skip("before", &before));
    }

    // The ctime clause of the racy guard, pinned SEPARATELY from the mtime clause.
    //
    // This test exists because the clause was previously deletable with the entire suite still green.
    // It is the clause that matters most in practice: any tool that restores mtime after writing
    // (`rsync -t`, `tar -x`, `cp -p`, Bazel, reproducible builds) leaves mtime permanently past the settle
    // margin, so ctime recency is then the ONLY thing standing between a same-size in-place rewrite and a
    // silent skip. `every_fingerprint_field_defeats_the_skip` covers ctime INEQUALITY; this covers ctime
    // RECENCY, which is a different property.
    #[test]
    fn recent_ctime_alone_defeats_the_skip() {
        let mut c = StatCache::new(SCAN);
        // mtime is ancient (as a tool that restores timestamps would leave it) but ctime is recent —
        // i.e. the bytes were just rewritten. Fingerprints match, so only the ctime margin can refuse.
        let k = key(10, OLD, SCAN - 1, 7);
        c.insert("a".into(), k);
        assert!(
            !c.may_skip("a", &k),
            "an old mtime with a RECENT ctime must re-hash: the content was just rewritten"
        );
        // and the same entry becomes skippable once the ctime is also settled
        let settled = key(10, OLD, OLD, 7);
        c.insert("b".into(), settled);
        assert!(c.may_skip("b", &settled));
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

        // A GENUINE v1 payload: StatKey had no nlink/mode, so a full parse of it fails. The version must
        // still be recognised (this is the real upgrade path, and it previously reported "corrupt").
        let v1 = serde_json::json!({
            "version": 1,
            "scan_started_ns": SCAN,
            "manifest_digest": "",
            "entries": { "a": {"size":10,"mtime_ns":1,"ctime_ns":1,"ino":7,"dev":1} }
        });
        let v1_bytes = serde_json::to_vec(&v1).unwrap();
        // The distinguishing property: the version is readable even though the FULL parse fails, which is
        // what lets an upgrade report "older version" instead of "corrupt". Asserting only that load()
        // returns empty passes against the one-stage loader too, so it pins nothing.
        assert!(
            serde_json::from_slice::<StatCache>(&v1_bytes).is_err(),
            "a v1 payload must indeed fail a full parse (else this test proves nothing)"
        );
        assert_eq!(
            StatCache::probe_version(&v1_bytes),
            Some(1),
            "the version must be readable WITHOUT a full parse"
        );
        std::fs::write(&p, &v1_bytes).unwrap();
        assert!(
            StatCache::load(&p).entries.is_empty(),
            "an older cache version → empty (full re-hash), not a hard error"
        );
    }

    #[test]
    fn save_then_load_roundtrips() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("nested/sc.json");
        let mut c = StatCache::new(SCAN);
        c.insert("dir/f.bin".into(), key(99, 1, 2, 3));
        c.save(&p).unwrap();
        let back = StatCache::load(&p);
        assert_eq!(back.scan_started_ns, SCAN);
        assert_eq!(back.entries.get("dir/f.bin"), c.entries.get("dir/f.bin"));
        assert!(
            !p.with_extension("tmp").exists(),
            "staging file renamed away"
        );
    }

    // The fingerprint must actually reflect a real file's stat, and change when the file changes.
    // The probe must PASS on an ordinary local filesystem (otherwise it would disable the skip
    // everywhere), leave no debris, and fail safe on a path it cannot write to.
    #[test]
    fn fidelity_probe_passes_on_a_normal_filesystem_and_cleans_up() {
        let d = tempfile::tempdir().unwrap();
        let before: Vec<_> = std::fs::read_dir(d.path()).unwrap().flatten().collect();
        match probe_fidelity(d.path()) {
            // Bounded on BOTH sides, and the lower bound is the load-bearing one. `>= 0` was vacuous —
            // `worst_gap_ns` starts at 0 and is a max of saturating subtractions, so it cannot be
            // negative — and its vacuity hid a real hazard: changing the adapter's clock to
            // `as_millis()` makes every measured gap 0, so `gap * 3 <= SETTLE_NS` always holds and the
            // probe degenerates into accept-everything, including on the 10s-granularity filesystems it
            // exists to refuse. One token, whole suite green. Measured on real local filesystems:
            // 24.6-47.1us, so the 1us floor has ~25x margin.
            Fidelity::Ok { observed_ns } => assert!(
                (1_000..SETTLE_NS).contains(&observed_ns),
                "implausible measured granularity {observed_ns}ns — check the adapter's clock units"
            ),
            // This arm is also what pins the adapter's FIELD CHOICE: because the probe backdates the file's
            // mtime each cycle, a build that measured `mtime_ns` instead of `ctime_ns` would see it frozen,
            // refuse, and land here — so this otherwise-ordinary "it passes on a normal FS" test is the one
            // that kills the `ctime -> mtime` mutation (a silent wrong-accept on frozen-ctime filesystems).
            Fidelity::Unusable => panic!("a local filesystem must support the skip's guards"),
        }
        let after: Vec<_> = std::fs::read_dir(d.path()).unwrap().flatten().collect();
        assert_eq!(before.len(), after.len(), "probe left debris behind");
    }

    // ---- the probe's MEASUREMENT, driven by a simulated clock ------------------------------------
    //
    // `fidelity_verdict` pins the DECISION, but until now nothing pinned the granularity measurement that
    // feeds it: reverting the `since_transition` timer left the whole suite green while silently
    // restoring the old accept curve (worst_gap pinned at ~20us, the x3 clause never firing). That is the
    // failure this codebase has shipped repeatedly — a mechanism revertible with a green suite.
    //
    // The fake filesystem is a TRUNCATION GRID: `ctime(t) = ((t + phase) / G) * G`, which is how a coarse
    // filesystem actually behaves. A "ctime advances by G after each write" model is NOT equivalent — it
    // would let a TRANSITIONS -> 1 mutation survive.
    const ITER_NS: i64 = 20_000; // cost of one rewrite+stat, matching the real loop

    /// Drive the COMPOSED probe (measure + verdict) against the same simulated filesystem. A4/A5/A6 go
    /// through this rather than calling `fidelity_verdict` by hand, so the composition is covered too.
    fn sim_verdict(g: i64, phase: i64) -> Fidelity {
        let now = std::cell::Cell::new(0i64);
        let ct = move |t: i64| ((t + phase) / g) * g;
        probe_verdict(
            || {
                now.set(now.get() + ITER_NS);
                ProbeStep::Ctime(ct(now.get()))
            },
            || now.get(),
            |ns| now.set(now.get() + ns),
        )
    }

    /// Drive `measure_granularity` against a simulated clock and a grid-truncating filesystem.
    fn sim(g: i64, phase: i64) -> (usize, i64) {
        let now = std::cell::Cell::new(0i64);
        let ct = move |t: i64| ((t + phase) / g) * g;
        measure_granularity(
            || {
                now.set(now.get() + ITER_NS);
                ProbeStep::Ctime(ct(now.get()))
            },
            || now.get(),
            |ns| now.set(now.get() + ns),
        )
    }

    // A1 — the measurement TRACKS granularity. Two-sided on purpose: the LOWER bound is the half that
    // kills the reverted timer (which reports ~20us at every G), a bare upper bound would pass at 0.
    #[test]
    fn measurement_tracks_granularity() {
        for g_ms in [100i64, 300, 600, 700, 800] {
            let g = g_ms * 1_000_000;
            for phase in [0, g / 4, g / 2, 3 * g / 4, g - 1] {
                let (seen, gap) = sim(g, phase);
                // Asserted, not `if`-guarded: a condition here would let the assertion below VANISH
                // rather than fail, which is the silent-skip shape this whole file exists to stamp out.
                assert!(
                    seen >= 2,
                    "G={g_ms}ms phase={phase}: only {seen} transitions"
                );
                assert!(
                    (gap - g).abs() <= 5_000_000,
                    "G={g_ms}ms phase={phase}: measured gap {gap}ns should track G={g}ns"
                );
            }
        }
    }

    // A2 — a normal filesystem. Upper bound ONLY: at microsecond granularity the loop correctly measures
    // its own ~20us iteration cost, so A1's two-sided bound must NOT be applied here.
    #[test]
    fn measurement_on_a_fine_grained_filesystem() {
        let (seen, gap) = sim(1_000, 0); // G = 1us
        assert_eq!(seen, TRANSITIONS);
        assert!(gap <= 1_000_000, "expected sub-millisecond, got {gap}ns");
    }

    // A3 — harness fidelity against the figures measured on REAL filesystems (build log O24/O25).
    #[test]
    fn simulated_clock_reproduces_real_filesystem_measurements() {
        let (_, gap100) = sim(100_000_000, 0);
        assert!(
            (gap100 - 100_000_000).abs() <= 3_000_000,
            "G=100ms -> {gap100}ns"
        );
        let (_, gap660) = sim(660_000_000, 0);
        assert!(
            (gap660 - 660_000_000).abs() <= 3_000_000,
            "G=660ms -> {gap660}ns"
        );
    }

    // A4 — G=700ms must be REFUSED, at EVERY phase. The second assertion is the load-bearing one: it
    // proves at least one phase reaches TRANSITIONS and is therefore refused BY THE GAP CLAUSE, not by the
    // deadline. Without it this test would silently degenerate into a deadline-only test the moment the
    // measurement window shifted, and the headroom mutations would survive it.
    #[test]
    fn coarse_granularity_is_refused_by_the_gap_not_the_deadline() {
        let g = 700_000_000i64;
        let mut gap_bound = 0;
        for i in 0..24 {
            let phase = g / 24 * i;
            let (seen, gap) = sim(g, phase);
            assert_eq!(
                sim_verdict(g, phase),
                Fidelity::Unusable,
                "phase={phase}: 700ms granularity must be refused (seen={seen} gap={gap})"
            );
            if seen == TRANSITIONS {
                gap_bound += 1;
            }
        }
        assert!(
            gap_bound > 0,
            "no phase reached {TRANSITIONS} transitions, so this test proved only that the DEADLINE \
             refuses 700ms — the gap clause went unexercised"
        );
    }

    // Acceptance must be a clean CUT in granularity, not a per-restart lottery: a filesystem must not be
    // accepted on one daemon start and refused on the next because of where in a ctime tick the daemon
    // happened to begin. That lottery (85% accept at 700ms, 50% at 800ms, 22% at 900ms) is the wart the x3
    // headroom was introduced to remove, and nothing tested it.
    //
    // The honest property is an INTERVAL one, not invariance. A hard threshold on a measurement quantized
    // to the poll quantum always leaves a boundary band; what matters is that the band is no wider than
    // that quantum, so it cannot span a granularity anyone would care about. An earlier version of this
    // test asserted flat phase-invariance and passed only because the ten granularities it sampled
    // straddled the real band (660ms and 670ms sit either side of it) — a global claim resting on sampling
    // luck, which is the vacuity this file exists to remove. So the bounds below are MEASURED here, not
    // asserted from constants.
    #[test]
    fn acceptance_is_a_clean_cut_in_granularity_not_a_per_restart_lottery() {
        let unanimous = |g: i64| -> Option<bool> {
            let first = matches!(sim_verdict(g, 0), Fidelity::Ok { .. });
            (1..24)
                .all(|i| matches!(sim_verdict(g, g / 24 * i), Fidelity::Ok { .. }) == first)
                .then_some(first)
        };

        // Everything comfortably below the x3 threshold (2s/3 = 666.7ms) is unanimously ACCEPTED, and
        // everything above it unanimously REFUSED, all the way out to a 10s-granularity filesystem.
        for g_ms in (1..=664).step_by(3) {
            assert_eq!(
                unanimous(g_ms * 1_000_000),
                Some(true),
                "G={g_ms}ms must be accepted at every phase"
            );
        }
        for g_us in (668_000..=10_000_000).step_by(97_000) {
            assert_eq!(
                unanimous(g_us * 1_000),
                Some(false),
                "G={g_us}us must be refused at every phase"
            );
        }

        // The undecided band between them must be at most one poll quantum wide — that is what makes it
        // a cut rather than a lottery.
        let (mut lo, mut hi) = (i64::MAX, i64::MIN);
        for g in (655_000_000..=680_000_000).step_by(100_000) {
            let (mut any_ok, mut any_no) = (false, false);
            for i in 0..24 {
                match sim_verdict(g, g / 24 * i) {
                    Fidelity::Ok { .. } => any_ok = true,
                    Fidelity::Unusable => any_no = true,
                }
            }
            if any_no {
                lo = lo.min(g);
            }
            if any_ok {
                hi = hi.max(g);
            }
        }
        let quantum = ITER_NS + PROBE_POLL_NS;
        assert!(
            hi > lo && hi - lo <= quantum,
            "undecided band [{lo}, {hi}] is {}ns wide, more than the {quantum}ns poll quantum — \
             acceptance has become phase-dependent over a range that matters",
            hi - lo
        );
    }

    // A5 — filesystems that are genuinely SAFE at a 2s settle margin must not be refused. This is what
    // rules out an over-strict multiplier (x8 would refuse 300ms).
    #[test]
    fn safe_granularities_are_accepted() {
        for g_ms in [300i64, 600] {
            let g = g_ms * 1_000_000;
            for phase in [0, g / 4, g / 2, 3 * g / 4, g - 1] {
                let (seen, gap) = sim(g, phase);
                assert!(
                    matches!(sim_verdict(g, phase), Fidelity::Ok { .. }),
                    "G={g_ms}ms phase={phase} is safe at a 2s margin and must be accepted \
                     (seen={seen} gap={gap})"
                );
            }
        }
    }

    // A6 — the documented worst case, at EVERY phase. The old probe false-passed here at ~20% because it
    // only needed to observe one tick inside its window; this asserts it away.
    #[test]
    fn very_coarse_granularity_is_refused_at_every_phase() {
        let g = 10_000_000_000i64; // 10s
        for i in 0..24 {
            let phase = g / 24 * i;
            let (seen, gap) = sim(g, phase);
            assert_eq!(
                sim_verdict(g, phase),
                Fidelity::Unusable,
                "G=10s phase={phase} must be refused (seen={seen} gap={gap})"
            );
        }
    }

    // A rewrite failure must abort IMMEDIATELY, not retry to the deadline: the assertion is that the
    // simulated clock barely advanced.
    #[test]
    fn rewrite_failure_aborts_without_burning_the_deadline() {
        let now = std::cell::Cell::new(0i64);
        let (seen, gap) = measure_granularity(
            || {
                now.set(now.get() + ITER_NS);
                ProbeStep::RewriteFailed
            },
            || now.get(),
            |ns| now.set(now.get() + ns),
        );
        assert_eq!((seen, gap), (0, 0));
        assert!(
            now.get() < SETTLE_NS / 100,
            "a rewrite failure must abort immediately, but the clock advanced {}ns",
            now.get()
        );
    }

    // A stat failure is transient and must RETRY until the deadline — the opposite of a rewrite failure.
    #[test]
    fn stat_failure_retries_until_the_deadline() {
        let now = std::cell::Cell::new(0i64);
        let (seen, _) = measure_granularity(
            || {
                now.set(now.get() + ITER_NS);
                ProbeStep::StatFailed
            },
            || now.get(),
            |ns| now.set(now.get() + ns),
        );
        assert_eq!(seen, 0);
        assert!(now.get() >= SETTLE_NS, "should have run to the deadline");
    }

    // A slow FIRST write must not be charged to the first measured interval. Creating and first-writing
    // the probe file can be far slower than steady state (cold page cache, allocation, journal commit); if
    // `since_transition` still points at loop entry when the baseline lands, that setup cost is measured as
    // granularity and a perfectly fine filesystem is REFUSED. This is the false-refusal direction, which
    // costs the whole stat-cache skip on every daemon start.
    #[test]
    fn a_slow_first_write_is_not_charged_to_the_first_interval() {
        let now = std::cell::Cell::new(0i64);
        let n = std::cell::Cell::new(0u32);
        let g = 1_000_000i64; // 1ms granularity: comfortably fine, must be accepted
        let step = || {
            n.set(n.get() + 1);
            now.set(now.get() + if n.get() == 1 { 800_000_000 } else { ITER_NS });
            ProbeStep::Ctime((now.get() / g) * g)
        };
        let v = probe_verdict(step, || now.get(), |ns| now.set(now.get() + ns));
        assert!(
            matches!(v, Fidelity::Ok { .. }),
            "an 800ms first write must not be measured as granularity, got {v:?}"
        );
    }

    // A filesystem whose ctime NEVER moves — vfat/exFAT (creation time only), SMB/CIFS, several FUSE
    // backends. This is the population the probe exists to refuse, and it pins three things no other test
    // does: that the baseline observation is not miscounted as a transition (seed it with a sentinel the
    // filesystem cannot return and this reports 1), that the loop terminates on the DEADLINE rather than
    // spinning forever, and that the end-to-end verdict is refusal.
    #[test]
    fn a_filesystem_whose_ctime_never_moves_is_refused() {
        let now = std::cell::Cell::new(0i64);
        let step = || {
            now.set(now.get() + ITER_NS);
            ProbeStep::Ctime(1_700_000_000_000_000_000) // a plausible, unmoving ctime
        };
        let (seen, gap) = measure_granularity(step, || now.get(), |ns| now.set(now.get() + ns));
        assert_eq!(seen, 0, "an unmoving ctime is not a transition");
        assert_eq!(gap, 0);
        assert!(
            now.get() >= SETTLE_NS && now.get() < SETTLE_NS * 2,
            "must stop at the deadline, not spin: clock at {}ns",
            now.get()
        );
        assert_eq!(fidelity_verdict(seen, gap), Fidelity::Unusable);
    }

    // The kept gap must be the WORST one, not the last. A filesystem that stalls once and then ticks
    // quickly has to be judged on the stall — with `worst_gap_ns = last` the 900ms stall is overwritten by
    // two ~20us ticks and the filesystem is ACCEPTED. The truncation grid above structurally CANNOT see
    // this: on a uniform grid every full interval equals G, so max == last in all 25 of its cases.
    #[test]
    fn the_worst_gap_is_kept_not_the_last_one() {
        let now = std::cell::Cell::new(0i64);
        let ctime = std::cell::Cell::new(0i64);
        let (seen, gap) = measure_granularity(
            || {
                now.set(now.get() + ITER_NS);
                if now.get() >= 900_000_000 {
                    ctime.set(ctime.get() + 1); // after the stall, a transition every step
                }
                ProbeStep::Ctime(ctime.get())
            },
            || now.get(),
            |ns| now.set(now.get() + ns),
        );
        assert_eq!(seen, TRANSITIONS);
        assert!(
            gap >= 900_000_000,
            "the 900ms stall must survive two fast ticks, got {gap}ns"
        );
        assert_eq!(fidelity_verdict(seen, gap), Fidelity::Unusable);
    }

    // Pins the probe's DECISION, which until now nothing could observe: the previous fix to this logic
    // could be reverted with the entire suite still green. Driving the verdict directly means no
    // coarse-granularity filesystem is needed to test it.
    #[test]
    fn fidelity_verdict_rejects_coarse_granularity() {
        // too few transitions seen -> never usable, whatever the gap
        assert_eq!(fidelity_verdict(2, 1_000), Fidelity::Unusable);
        // a normal filesystem: microsecond granularity
        assert!(matches!(fidelity_verdict(3, 20_000), Fidelity::Ok { .. }));
        // 300 ms is comfortably safe at a 2 s settle margin and must NOT be refused (x8 would refuse it)
        assert!(matches!(
            fidelity_verdict(3, 300_000_000),
            Fidelity::Ok { .. }
        ));
        // 700 ms must be refused. This is the assertion that pins the multiplier: at x2 (or the bare
        // inequality) 700 ms passes, so weakening it fails here.
        assert_eq!(fidelity_verdict(3, 700_000_000), Fidelity::Unusable);
        // the documented data-loss case
        assert_eq!(fidelity_verdict(3, 1_000_000_000), Fidelity::Unusable);
        // absurd values saturate toward refusal rather than wrapping into acceptance
        assert_eq!(fidelity_verdict(3, i64::MAX), Fidelity::Unusable);
    }

    #[test]
    fn fidelity_probe_fails_safe_on_an_unwritable_path() {
        assert_eq!(
            probe_fidelity(Path::new("/definitely/not/a/writable/dir")),
            Fidelity::Unusable
        );
    }

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

/// Result of probing the workspace filesystem for the timestamp behaviour the skip depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fidelity {
    /// ctime demonstrably moved in response to an in-place rewrite, within the settle margin. The skip's
    /// guards mean what they say on this filesystem.
    Ok { observed_ns: i64 },
    /// ctime did not move within the margin — either it does not track writes at all (SMB/CIFS, vfat and
    /// exFAT report a creation time; several FUSE backends likewise) or the timestamp granularity is
    /// coarser than the margin. Either way the racy-window guard is not sound here and the skip must be
    /// refused.
    Unusable,
}

/// One iteration's observation of the probe file.
///
/// Three-state, not `Option`: a REWRITE failure means the filesystem is unusable and must abort
/// immediately, while a STAT failure is transient and must retry until the deadline. Collapsing them
/// would turn a failing filesystem into a full `SETTLE_NS` startup stall plus ~1000 futile writes.
enum ProbeStep {
    Ctime(i64),
    StatFailed,
    RewriteFailed,
}

/// How long to wait between polls when ctime has not moved.
const PROBE_POLL_NS: i64 = 2_000_000; // 2ms

/// Consecutive ctime transitions the probe must observe. Reaching this many requires TWO FULL
/// inter-transition intervals inside the deadline, which is what bounds granularity on its own — the
/// first interval is a partial and does not count. Hence >= 2 is load-bearing.
const TRANSITIONS: usize = 3;

/// The probe's measurement loop, with its I/O and clock INJECTED so it can be driven by a simulated
/// clock against a simulated coarse-granularity filesystem.
///
/// `SETTLE_NS` and `TRANSITIONS` are read here rather than passed in, deliberately. Parameterising them
/// would move the policy into an untested argument list, where the real adapter could pass `i64::MAX`
/// and `1` with every unit test still green — the same "the guard is one level up and unobserved" defect
/// this function has already shipped twice. A simulated clock makes real-scale constants free, so there
/// is no testability reason to hoist them.
///
/// Returns `(transitions_seen, worst_gap_ns)`. A rewrite failure returns `(0, 0)` immediately, which
/// `fidelity_verdict` reads as unusable.
fn measure_granularity(
    mut step: impl FnMut() -> ProbeStep,
    now_ns: impl Fn() -> i64,
    mut sleep_ns: impl FnMut(i64),
) -> (usize, i64) {
    let start = now_ns();
    // The baseline ctime is taken by this loop from its OWN first step — `None` until then — not supplied
    // by the caller. Passing it in was a second policy seam: seed it with anything the filesystem cannot
    // return (0, say) and iteration one becomes a spurious transition, so the probe needs only two real
    // ones — one full interval, silently halving the invariant below. There is now nothing to pass, and
    // the baseline is taken at the single site below rather than by a duplicate `match` up here.
    let mut prev: Option<i64> = None;
    let mut worst_gap_ns: i64 = 0;
    let mut seen = 0usize;
    // Time since the LAST ctime transition — that interval IS the granularity. Initialised BEFORE the
    // loop, so the first interval is a partial and only the 2nd and 3rd are full: that is why
    // TRANSITIONS >= 2 is load-bearing. An earlier version reset this every iteration, so it measured a
    // single rewrite (~20us) and the headroom check could never fire.
    let mut since_transition = now_ns();
    while seen < TRANSITIONS && now_ns().saturating_sub(start) < SETTLE_NS {
        match step() {
            ProbeStep::RewriteFailed => return (0, 0),
            // First observation: the baseline. NOT a transition — counting it would fabricate one and
            // leave only one full inter-transition interval measured.
            ProbeStep::Ctime(c) if prev.is_none() => {
                prev = Some(c);
                since_transition = now_ns();
                continue;
            }
            ProbeStep::Ctime(c) if prev != Some(c) => {
                worst_gap_ns = worst_gap_ns.max(now_ns().saturating_sub(since_transition));
                since_transition = now_ns();
                prev = Some(c);
                seen += 1;
                continue; // no sleep after a transition
            }
            _ => {}
        }
        sleep_ns(PROBE_POLL_NS);
    }
    (seen, worst_gap_ns)
}

/// Measure, then judge. Composed HERE rather than in `probe_fidelity`, so that the composition itself is
/// covered by the simulated-clock tests. Left in the adapter, `fidelity_verdict(seen, seen, gap)` was a
/// one-token edit that made the probe accept a filesystem whose ctime never moves at all — with the whole
/// suite green. The adapter is now pure I/O, holding no policy and no composition.
fn probe_verdict(
    step: impl FnMut() -> ProbeStep,
    now_ns: impl Fn() -> i64,
    sleep_ns: impl FnMut(i64),
) -> Fidelity {
    let (seen, worst_gap_ns) = measure_granularity(step, now_ns, sleep_ns);
    fidelity_verdict(seen, worst_gap_ns)
}

/// Probe `dir`'s filesystem for the ONE property the skip's safety actually rests on: that an in-place
/// rewrite moves `ctime` within the settle margin.
///
/// This exists because the alternative is an unenforceable deployment assumption. The guards are sound on
/// node-local Linux/XFS/ext4 and were DEMONSTRATED to lose data on a 1 s-granularity filesystem — and
/// nothing stops an operator pointing `--tree` at an EFS/Azure-Files/gcsfuse PVC, which is an entirely
/// ordinary k8s choice. A file whose mtime has been normalised by `tar -x`/`rsync -t`/Bazel is
/// permanently past the settle margin, so on such a filesystem ctime is the ONLY remaining defence; if it
/// is frozen too, a same-size in-place rewrite is invisible.
///
/// One rewrite loop, bounded by the settle margin, run once at startup. Fails toward `Unusable`.
pub fn probe_fidelity(dir: &Path) -> Fidelity {
    use std::io::Write;
    // Sweep debris from any previous run FIRST. This probe writes into the agent's workspace before the
    // daemon has registered signal handlers, so a SIGTERM in that window kills the process with default
    // disposition and leaves the file behind — after which every subsequent start fails materialize's
    // empty-tree precondition. A stranded diagnostic file must not be able to cause a permanent
    // CrashLoopBackOff.
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            if e.file_name()
                .to_string_lossy()
                .starts_with(".capsule-fidelity-probe-")
            {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let probe = dir.join(format!(".capsule-fidelity-probe-{}", std::process::id()));
    let cleanup = |f: &Path| {
        let _ = std::fs::remove_file(f);
    };
    let stat = |f: &Path| {
        std::fs::metadata(f)
            .map(|m| StatKey::from_metadata(&m))
            .ok()
    };

    // Rewrite IN PLACE over an open fd — not `fs::write`, which truncates. The threat being modelled is a
    // same-size overwrite, and a backend can plausibly stamp ctime on truncate but not on a pure overwrite.
    //
    // Then backdate the file's mtime to a fixed old instant. This makes the probe validate the EXACT
    // property `may_skip` leans on — that CTIME advances on an in-place rewrite EVEN WHEN mtime is pushed
    // backward — rather than a proxy for it. On a frozen-ctime filesystem (vfat/exFAT/SMB report a creation
    // time; mtime there is live and fine-grained) the loop reads ctime, sees it frozen, and refuses; a
    // build that mistakenly read mtime instead would see it pinned at this old value, never transition, and
    // also refuse — so reading the wrong field is no longer silently safe on a normal filesystem, where
    // mtime and ctime otherwise move together and hide the mistake. `set_times` itself also bumps ctime, so
    // the guarantee widens from "ctime moves on a pure overwrite" to "…on an overwrite or a utimes" — inert,
    // because a backend that stamps ctime on `utimensat` but not on `write()` inverts the usual
    // metadata-strength ordering and is not a real backend.
    //
    // BEST-EFFORT, deliberately: the `set_times` result is dropped. A backend without `utimensat` must not
    // turn a working filesystem into a permanent false-refuse (which would disable the skip everywhere on
    // it); a set_times failure can only weaken the wrong-field guard on that exotic backend, never flip the
    // real ctime verdict. Folding it into `rewrite`'s success would reintroduce exactly that false-refuse.
    let rewrite = |f: &Path, byte: u8| -> bool {
        let Ok(mut fh) = std::fs::OpenOptions::new().write(true).open(f) else {
            return false;
        };
        if fh.write_all(&[byte; 4096]).is_err() {
            return false;
        }
        let _ = fh.set_times(
            std::fs::FileTimes::new()
                .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(100)),
        );
        true
    };
    if std::fs::write(&probe, [0u8; 4096]).is_err() {
        // `fs::write` creates then writes, so an ENOSPC/EIO after create leaves the file behind — inside
        // the agent's workspace, where the next publish would pick it up into the manifest.
        cleanup(&probe);
        return Fidelity::Unusable;
    }
    // Fail fast: the loop treats a stat failure as transient and retries to the deadline, so without this
    // a filesystem where stat is broken outright would stall startup for the full SETTLE_NS. It no longer
    // seeds the measurement — the loop takes its own baseline.
    if stat(&probe).is_none() {
        cleanup(&probe);
        return Fidelity::Unusable;
    }

    // Thin real-I/O adapter over `measure_granularity` — all policy lives in there.
    let t0 = std::time::Instant::now();
    let mut byte = 1u8;
    let verdict = probe_verdict(
        || {
            if !rewrite(&probe, byte) {
                return ProbeStep::RewriteFailed;
            }
            byte = byte.wrapping_add(1);
            match stat(&probe) {
                Some(k) => ProbeStep::Ctime(k.ctime_ns),
                None => ProbeStep::StatFailed,
            }
        },
        || t0.elapsed().as_nanos() as i64,
        |ns| std::thread::sleep(std::time::Duration::from_nanos(ns.max(0) as u64)),
    );
    cleanup(&probe);
    verdict
}

/// The probe's decision, split out from the I/O so it can be tested without a coarse-granularity
/// filesystem to hand. The previous fix to this logic shipped with NO test able to observe it — reverting
/// it left the whole suite green — which is the exact failure this codebase keeps repeating.
///
/// Safety needs G < `SETTLE_NS`: a write during a scan truncates mtime down by at most G, and the settle
/// margin has to cover that.
///
/// Note what already bounds G before this test: reaching `want` transitions requires two FULL
/// inter-transition intervals inside the loop's `SETTLE_NS` deadline, so `seen == want` alone implies
/// G ≲ 1 s. A ×2 test here (G ≤ `SETTLE_NS`/2 = 1 s) is therefore exactly EQUIVALENT to the bare
/// inequality — it decides nothing, which is precisely what a previous comment here claimed it did. ×3 is
/// the smallest multiplier that actually binds: it rejects G > ~666 ms outright, which also removes a real
/// wart — in the band the deadline alone leaves undecided, the SAME filesystem was accepted or rejected at
/// random across daemon restarts. ×8 would be too strict, refusing a 300 ms filesystem that is comfortably
/// safe at a 2 s margin.
fn fidelity_verdict(seen: usize, worst_gap_ns: i64) -> Fidelity {
    if seen == TRANSITIONS && worst_gap_ns.saturating_mul(3) <= SETTLE_NS {
        Fidelity::Ok {
            observed_ns: worst_gap_ns,
        }
    } else {
        Fidelity::Unusable
    }
}
