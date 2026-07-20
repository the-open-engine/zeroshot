/**
 * LessonStore - LYO's durable lesson library (lesson-delta learning layer, v0.1)
 *
 * LYO is the SOLE WRITER of this store: it never shares a run ledger's DB
 * (design doc §1.4, Appendix B.3). Implements:
 * - §3 schema (append-only lesson_delta log + folded lesson state + attribution
 *   join table lesson_application)
 * - §4.1 v_lesson_library view + §4.2 Thompson-sampling selection
 * - §5.1 validation-grounded counter rule + §5.2 Wilson status rules
 * - §5.3 lesson_decision log: per-decision candidate snapshot (alpha/beta at
 *   decision time) + Monte-Carlo selection propensities (v0.2). This is the
 *   logged-bandit data the ratio-lift estimator joins against outcomes.
 * - §6 replay (fold deltas back into state)
 * - §7 curator (merge / prune, watermark-driven)
 *
 * Documented deviations from the design doc (each also noted inline):
 *  1. lesson_application adds trigger_message_id and its uniqueness is
 *     UNIQUE(lesson_id, run_id, trigger_message_id) instead of
 *     UNIQUE(lesson_id, run_id). Rationale: a Zeroshot "run" (cluster) contains
 *     multiple validation cycles (rejection -> intervention -> next
 *     validation); the grounded attribution unit is the cycle, not the run.
 *     run_id = cluster_id remains for provenance/metrics.
 *  2. Promotion (candidate -> active) is applied to the lesson row WITHOUT
 *     emitting a delta (the design's delta_type list has no PROMOTE; §6 replay
 *     recomputes status from counters during fold). QUARANTINE / RETIRE /
 *     MERGE_INTO / REINSTATE ARE deltas.
 */

const crypto = require('crypto');
const Database = require('better-sqlite3');
const { normalizeCue } = require('./failure-classifier');

const WILSON_Z = 1.96;
const STATUS_RULE_MIN_SAMPLES = 8;
const PROMOTION_WILSON_LOWER = 0.5;
const QUARANTINE_WILSON_UPPER = 0.45;

// DEVIATION 1 (see file header): lesson_application carries trigger_message_id
// and is UNIQUE(lesson_id, run_id, trigger_message_id) — the attribution unit
// is the validation cycle (one injection per trigger message), not the run.
const DDL = `
CREATE TABLE IF NOT EXISTS lesson_delta (
  delta_id    INTEGER PRIMARY KEY AUTOINCREMENT,
  lesson_id   TEXT NOT NULL,              -- lesson this delta mutates
  run_id      TEXT,                       -- provenance run; NULL for curator passes
  ts          TEXT NOT NULL DEFAULT (datetime('now')),
  actor       TEXT NOT NULL,              -- 'reflector' | 'validator-rule' | 'curator'
  delta_type  TEXT NOT NULL,              -- CREATE | EDIT | MARK_HELPFUL | MARK_HARMFUL
                                          -- MERGE_INTO | QUARANTINE | REINSTATE | RETIRE
  payload     TEXT NOT NULL               -- JSON; per-type shape documented in methods
);

CREATE TABLE IF NOT EXISTS lesson (
  lesson_id     TEXT PRIMARY KEY,
  status        TEXT NOT NULL DEFAULT 'candidate',  -- candidate | active | quarantined | retired
  failure_class TEXT NOT NULL,
  trigger_cue   TEXT NOT NULL,
  explanation   TEXT NOT NULL,
  intervention  TEXT NOT NULL,
  helpful_count INTEGER NOT NULL DEFAULT 0,
  harmful_count INTEGER NOT NULL DEFAULT 0,
  uses          INTEGER NOT NULL DEFAULT 0,
  created_at    TEXT NOT NULL,
  updated_at    TEXT NOT NULL,
  provenance    TEXT NOT NULL DEFAULT '[]'      -- JSON array of run_ids
);

CREATE TABLE IF NOT EXISTS lesson_application (
  application_id     TEXT PRIMARY KEY,
  lesson_id          TEXT NOT NULL REFERENCES lesson(lesson_id),
  run_id             TEXT NOT NULL,
  trigger_message_id TEXT,
  task_cue           TEXT,                  -- what matched at retrieval time
  sampled_score      REAL,                  -- the Thompson draw that selected it (audit)
  outcome            TEXT NOT NULL DEFAULT 'pending',  -- pending | passed | failed
  counted            INTEGER NOT NULL DEFAULT 0,       -- 1 once folded into counters
  UNIQUE(lesson_id, run_id, trigger_message_id)
);

CREATE TABLE IF NOT EXISTS lyo_meta (key TEXT PRIMARY KEY, value TEXT);

-- Preference-pair learning evidence (ported from lyo-kernel recordTrace /
-- recordPreferencePair semantics). These are plain evidence tables, NOT
-- lesson deltas: they record which behavior trace was preferred over which,
-- so a future reflector can turn audited preferences into lessons.
CREATE TABLE IF NOT EXISTS learning_trace (
  trace_id     TEXT PRIMARY KEY,
  run_id       TEXT,
  kind         TEXT NOT NULL CHECK (kind IN ('behavior', 'protocol_application', 'agent_response', 'tool_use', 'other')),
  summary      TEXT NOT NULL,
  ref          TEXT,
  payload_json TEXT,
  created_at   TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS preference_pair (
  preference_id     TEXT PRIMARY KEY,
  context_hash      TEXT NOT NULL,
  chosen_trace_id   TEXT NOT NULL REFERENCES learning_trace(trace_id),
  rejected_trace_id TEXT NOT NULL REFERENCES learning_trace(trace_id),
  reason            TEXT NOT NULL,
  evidence_ref      TEXT NOT NULL,
  recorded_by       TEXT,
  confidence        TEXT NOT NULL DEFAULT 'medium' CHECK (confidence IN ('low', 'medium', 'high')),
  created_at        TEXT NOT NULL,
  CHECK (chosen_trace_id <> rejected_trace_id)
);

-- §5.3 decision log (v0.2). One row per intervention decision: every
-- candidate's posterior parameters (alpha = helpful+1, beta = harmful+1) and
-- selection propensity at decision time, the selected arms with their
-- Thompson draws, and the null-arm indicator (1 = no candidate existed, the
-- decision was "inject no lesson"). Immutable once written.
CREATE TABLE IF NOT EXISTS lesson_decision (
  decision_id        TEXT PRIMARY KEY,
  run_id             TEXT NOT NULL,
  trigger_message_id TEXT,
  cycle_index        INTEGER,
  failure_class      TEXT NOT NULL,
  task_cue           TEXT,
  candidates         TEXT NOT NULL,   -- JSON [{lesson_id, alpha, beta, propensity}]
  selected           TEXT NOT NULL,   -- JSON [{lesson_id, theta}]
  null_arm           REAL NOT NULL DEFAULT 0,
  context            TEXT NOT NULL DEFAULT '{}',
  created_at         TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_delta_lesson ON lesson_delta(lesson_id, delta_id);
CREATE INDEX IF NOT EXISTS idx_app_run      ON lesson_application(run_id, counted);
CREATE INDEX IF NOT EXISTS idx_lesson_class ON lesson(failure_class, status);
CREATE INDEX IF NOT EXISTS idx_decision_run   ON lesson_decision(run_id);
CREATE INDEX IF NOT EXISTS idx_decision_class ON lesson_decision(failure_class);

-- §4.1 the library view. Candidates stay retrievable for exploration.
CREATE VIEW IF NOT EXISTS v_lesson_library AS
SELECT lesson_id, failure_class, trigger_cue, explanation, intervention,
  helpful_count, harmful_count, uses,
  CAST(helpful_count + 1 AS REAL) / (helpful_count + harmful_count + 2) AS posterior_mean
FROM lesson
WHERE status IN ('active', 'candidate');
`;

function randomId(prefix) {
  // les_<16 hex> / app_<16 hex>
  return `${prefix}_${crypto.randomBytes(8).toString('hex')}`;
}

function nowIso() {
  return new Date().toISOString();
}

function sha256(text) {
  return crypto.createHash('sha256').update(text).digest('hex');
}

// --- Thompson sampling helpers (§4.2) ---------------------------------------

// Box-Muller standard normal from an injectable uniform rng.
function sampleStandardNormal(rng) {
  let u = 0;
  while (u === 0) u = rng(); // guard log(0)
  return Math.sqrt(-2 * Math.log(u)) * Math.cos(2 * Math.PI * rng());
}

// Marsaglia-Tsang gamma sampler. Local implementation, no new dependencies.
function sampleGamma(shape, rng) {
  if (shape <= 0) {
    throw new Error(`sampleGamma: shape must be > 0, got ${shape}`);
  }
  if (shape < 1) {
    // Boost: Gamma(k) = Gamma(k + 1) * U^(1/k)
    return sampleGamma(shape + 1, rng) * Math.pow(rng(), 1 / shape);
  }
  const d = shape - 1 / 3;
  const c = 1 / Math.sqrt(9 * d);
  for (;;) {
    let x;
    let v;
    do {
      x = sampleStandardNormal(rng);
      v = 1 + c * x;
    } while (v <= 0);
    v = v * v * v;
    const u = rng();
    if (u <= 0) continue;
    if (u < 1 - 0.0331 * x * x * x * x) return d * v;
    if (Math.log(u) < 0.5 * x * x + d * (1 - v + Math.log(v))) return d * v;
  }
}

// §5.2 Wilson score interval with z = 1.96.
function wilsonInterval(helpful, n, z = WILSON_Z) {
  const phat = helpful / n;
  const z2 = z * z;
  const denominator = 1 + z2 / n;
  const center = phat + z2 / (2 * n);
  const margin = z * Math.sqrt((phat * (1 - phat)) / n + z2 / (4 * n * n));
  return {
    lower: (center - margin) / denominator,
    upper: (center + margin) / denominator,
  };
}

// Pure status-rule fold shared by applyStatusRules (DB path) and replayLesson
// (reconstruction path). Mutates and returns the given state object.
function foldStatusRules(state) {
  const n = state.helpful_count + state.harmful_count;
  if (n < STATUS_RULE_MIN_SAMPLES) return state;
  const { lower, upper } = wilsonInterval(state.helpful_count, n);
  if (lower > PROMOTION_WILSON_LOWER && state.status === 'candidate') {
    state.status = 'active';
  }
  if (upper < QUARANTINE_WILSON_UPPER && state.status !== 'quarantined') {
    state.status = 'quarantined';
  }
  return state;
}

function unionProvenance(provenance, runId) {
  if (runId && !provenance.includes(runId)) {
    provenance.push(runId);
  }
  return provenance;
}

class LessonStore {
  constructor(dbPath = ':memory:') {
    this.dbPath = dbPath;
    this._closed = false;

    // Mirror Ledger's pragma setup (src/ledger.js): WAL journal for concurrent
    // readers, synchronous NORMAL, busy timeout 5000ms (env-overridable).
    const busyTimeoutMs = (() => {
      const raw = process.env.ZEROSHOT_SQLITE_BUSY_TIMEOUT_MS;
      if (!raw) return 5000;
      const value = Number(raw);
      return Number.isFinite(value) && value >= 0 ? value : 5000;
    })();

    this.db = new Database(dbPath, { timeout: busyTimeoutMs });
    this._initSchema();
  }

  _initSchema() {
    this.db.pragma('journal_mode = WAL');
    this.db.pragma('synchronous = NORMAL');
    this.db.pragma('wal_autocheckpoint = 1000');
    this.db.exec(DDL);
    this._migrate();
  }

  // Idempotent migrations. v2 (§5.3): lesson_application gains decision_id,
  // the join key into the lesson_decision log. The DDL intentionally keeps
  // the v1 lesson_application shape so fresh and pre-v0.2 databases converge
  // through this ONE path; pre-existing application rows keep NULL (they
  // predate the decision log). schema_version is upserted every open.
  _migrate() {
    const applicationColumns = this.db
      .prepare('PRAGMA table_info(lesson_application)')
      .all()
      .map((column) => column.name);
    if (!applicationColumns.includes('decision_id')) {
      this.db.exec('ALTER TABLE lesson_application ADD COLUMN decision_id TEXT');
    }
    this._setMeta('schema_version', '2');
  }

  close() {
    if (this._closed) return;
    this._closed = true;
    this.db.close();
  }

  getLesson(lessonId) {
    return this.db.prepare('SELECT * FROM lesson WHERE lesson_id = ?').get(lessonId) || null;
  }

  getDeltas(lessonId) {
    return this.db
      .prepare('SELECT * FROM lesson_delta WHERE lesson_id = ? ORDER BY delta_id')
      .all(lessonId);
  }

  _getMeta(key) {
    const row = this.db.prepare('SELECT value FROM lyo_meta WHERE key = ?').get(key);
    return row ? row.value : null;
  }

  _setMeta(key, value) {
    this.db
      .prepare(
        'INSERT INTO lyo_meta (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value'
      )
      .run(key, value);
  }

  _emitDelta({ lesson_id, run_id = null, actor, delta_type, payload }) {
    const info = this.db
      .prepare(
        'INSERT INTO lesson_delta (lesson_id, run_id, actor, delta_type, payload) VALUES (?, ?, ?, ?, ?)'
      )
      .run(lesson_id, run_id ?? null, actor, delta_type, JSON.stringify(payload ?? {}));
    return Number(info.lastInsertRowid);
  }

  /**
   * Create-or-merge a lesson. If a lesson with the same failure_class AND
   * identical normalized trigger_cue exists with status candidate/active, emit
   * an EDIT delta (append run_id to provenance, bump updated_at) and return the
   * existing lesson; otherwise emit a CREATE delta and insert a new candidate.
   */
  createLesson({ failure_class, trigger_cue, explanation, intervention, run_id, actor }) {
    if (!failure_class) {
      throw new Error('LessonStore.createLesson: failure_class is required');
    }
    const cue = normalizeCue(trigger_cue);
    const now = nowIso();
    const lessonActor = actor || 'reflector';

    const createOrMerge = this.db.transaction(() => {
      const existing = this.db
        .prepare(
          `SELECT * FROM lesson
           WHERE failure_class = ? AND trigger_cue = ? AND status IN ('candidate', 'active')`
        )
        .get(failure_class, cue);

      if (existing) {
        // EDIT merge: provenance + updated_at only. The explanation /
        // intervention / trigger_cue text is NEVER rewritten (ACE
        // brevity-bias/context-collapse rule, design doc §7); the reflector's
        // proposed text is kept in the delta payload for audit only.
        this._emitDelta({
          lesson_id: existing.lesson_id,
          run_id,
          actor: lessonActor,
          delta_type: 'EDIT',
          payload: { run_id: run_id ?? null, updated_at: now, explanation, intervention },
        });
        const provenance = unionProvenance(JSON.parse(existing.provenance), run_id);
        this.db
          .prepare('UPDATE lesson SET provenance = ?, updated_at = ? WHERE lesson_id = ?')
          .run(JSON.stringify(provenance), now, existing.lesson_id);
        return { ...existing, provenance: JSON.stringify(provenance), updated_at: now };
      }

      const lessonId = randomId('les');
      const provenance = run_id ? [run_id] : [];
      this._emitDelta({
        lesson_id: lessonId,
        run_id,
        actor: lessonActor,
        delta_type: 'CREATE',
        payload: {
          failure_class,
          trigger_cue: cue,
          explanation: explanation ?? '',
          intervention: intervention ?? '',
          created_at: now,
          updated_at: now,
          provenance,
        },
      });
      this.db
        .prepare(
          `INSERT INTO lesson (
             lesson_id, status, failure_class, trigger_cue, explanation, intervention,
             helpful_count, harmful_count, uses, created_at, updated_at, provenance
           ) VALUES (?, 'candidate', ?, ?, ?, ?, 0, 0, 0, ?, ?, ?)`
        )
        .run(
          lessonId,
          failure_class,
          cue,
          explanation ?? '',
          intervention ?? '',
          now,
          now,
          JSON.stringify(provenance)
        );
      return this.getLesson(lessonId);
    });

    return createOrMerge();
  }

  /**
   * §4.2 retrieval + Thompson selection. For each library lesson of the given
   * failure_class draw theta ~ Beta(helpful+1, harmful+1) via two gamma draws,
   * sort by theta desc, take the top `limit`, annotated with sampled_score.
   * The injectable rng makes tests deterministic.
   */
  selectLessons({ failure_class, limit = 2, rng = Math.random }) {
    const rows = this.db
      .prepare('SELECT * FROM v_lesson_library WHERE failure_class = ?')
      .all(failure_class);
    const scored = rows.map((row) => {
      const g1 = sampleGamma(row.helpful_count + 1, rng);
      const g2 = sampleGamma(row.harmful_count + 1, rng);
      return { ...row, sampled_score: g1 / (g1 + g2) };
    });
    scored.sort((a, b) => b.sampled_score - a.sampled_score);
    return scored.slice(0, Math.max(0, limit));
  }

  /**
   * §4.2 selection with §5.3 decision-record data. Same Thompson policy as
   * selectLessons, but additionally returns the FULL candidate set annotated
   * with the posterior parameters (alpha = helpful+1, beta = harmful+1) and
   * the selection propensity of each candidate: P(lesson lands in the top
   * `limit`) under the policy, Monte-Carlo estimated with `propensityReplicates`
   * replicates of the same injectable rng (the propensities feed the
   * ratio-lift estimator's inverse-propensity weighting). When every
   * candidate fits in the limit, inclusion is certain and propensity is
   * exactly 1 (MC loop skipped). null_arm is 1 only when no candidate exists
   * (in practice createLesson runs first, so at least one always does).
   */
  selectWithDecision({ failure_class, limit = 2, rng = Math.random, propensityReplicates = 1000 }) {
    const rows = this.db
      .prepare('SELECT * FROM v_lesson_library WHERE failure_class = ?')
      .all(failure_class);

    if (rows.length === 0) {
      return { selected: [], candidates: [], null_arm: 1 };
    }

    const drawThetas = () =>
      rows.map((row) => {
        const g1 = sampleGamma(row.helpful_count + 1, rng);
        const g2 = sampleGamma(row.harmful_count + 1, rng);
        return g1 / (g1 + g2);
      });

    // Indices of the top-`limit` thetas, in descending-theta order.
    const topIndices = (thetas) =>
      thetas
        .map((theta, index) => ({ theta, index }))
        .sort((a, b) => b.theta - a.theta)
        .slice(0, Math.max(0, limit))
        .map((entry) => entry.index);

    const inclusionCertain = rows.length <= limit;
    const replicates = Math.max(0, propensityReplicates);
    const tallies = new Array(rows.length).fill(0);
    if (!inclusionCertain) {
      for (let replicate = 0; replicate < replicates; replicate++) {
        for (const index of topIndices(drawThetas())) {
          tallies[index]++;
        }
      }
    }

    const candidates = rows.map((row, index) => ({
      lesson_id: row.lesson_id,
      alpha: row.helpful_count + 1,
      beta: row.harmful_count + 1,
      propensity: inclusionCertain ? 1 : replicates > 0 ? tallies[index] / replicates : 0,
    }));

    // The real selection draw (independent of the MC replicates).
    const thetas = drawThetas();
    const selected = topIndices(thetas).map((index) => ({
      ...rows[index],
      sampled_score: thetas[index],
    }));

    return { selected, candidates, null_arm: 0 };
  }

  /**
   * §2/§4.2 step 4: one application row per injected lesson (outcome pending).
   * INSERT OR IGNORE on UNIQUE(lesson_id, run_id, trigger_message_id); uses is
   * bumped only when a row was actually inserted. Returns the application row
   * (new or existing). NOTE: NULL trigger_message_id never dedupes (SQLite
   * treats NULLs as distinct); callers should always pass the trigger id.
   * decision_id (v0.2) joins the application to its lesson_decision row.
   */
  recordApplication({
    lesson_id,
    run_id,
    trigger_message_id,
    task_cue,
    sampled_score,
    decision_id,
  }) {
    const applicationId = randomId('app');
    const record = this.db.transaction(() => {
      const info = this.db
        .prepare(
          `INSERT OR IGNORE INTO lesson_application
             (application_id, lesson_id, run_id, trigger_message_id, task_cue, sampled_score, decision_id, outcome, counted)
           VALUES (?, ?, ?, ?, ?, ?, ?, 'pending', 0)`
        )
        .run(
          applicationId,
          lesson_id,
          run_id,
          trigger_message_id ?? null,
          task_cue ?? null,
          sampled_score ?? null,
          decision_id ?? null
        );

      if (info.changes > 0) {
        this.db.prepare('UPDATE lesson SET uses = uses + 1 WHERE lesson_id = ?').run(lesson_id);
        return this.db
          .prepare('SELECT * FROM lesson_application WHERE application_id = ?')
          .get(applicationId);
      }

      return this.db
        .prepare(
          `SELECT * FROM lesson_application
           WHERE lesson_id = ? AND run_id = ? AND trigger_message_id IS ?`
        )
        .get(lesson_id, run_id, trigger_message_id ?? null);
    });

    return record();
  }

  /**
   * §5.3 decision log: one immutable row per intervention decision, capturing
   * every candidate's (alpha, beta, propensity) at decision time plus the
   * selected arms and their Thompson draws. Joined against outcomes via
   * lesson_application.decision_id, this is the logged-bandit dataset the
   * ratio-lift estimator (§5.3) evaluates — including the null arm (cycles
   * where no lesson was injected). decision_id is dec_<16 hex>.
   */
  recordDecision({
    run_id,
    trigger_message_id = null,
    cycle_index = null,
    failure_class,
    task_cue = null,
    candidates,
    selected,
    null_arm = 0,
    context = {},
  }) {
    if (!run_id || !failure_class) {
      throw new Error('LessonStore.recordDecision: run_id and failure_class are required');
    }
    if (!Array.isArray(candidates) || !Array.isArray(selected)) {
      throw new Error('LessonStore.recordDecision: candidates and selected must be arrays');
    }
    const decisionId = randomId('dec');
    this.db
      .prepare(
        `INSERT INTO lesson_decision
           (decision_id, run_id, trigger_message_id, cycle_index, failure_class, task_cue,
            candidates, selected, null_arm, context, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`
      )
      .run(
        decisionId,
        run_id,
        trigger_message_id ?? null,
        cycle_index ?? null,
        failure_class,
        task_cue ?? null,
        JSON.stringify(candidates),
        JSON.stringify(selected),
        null_arm ? 1 : 0,
        JSON.stringify(context ?? {}),
        nowIso()
      );
    return this.getDecision(decisionId);
  }

  getDecision(decisionId) {
    return (
      this.db.prepare('SELECT * FROM lesson_decision WHERE decision_id = ?').get(decisionId) || null
    );
  }

  /**
   * §5.1 the validation-grounded counter rule. Counters move ONLY through
   * actual injection rows (lesson_application); a lesson with no application
   * row for the run never moves (Huang et al. 2023: self-assessment proposes,
   * the environment counts).
   */
  applyValidationOutcome({ run_id, outcome }) {
    if (outcome !== 'passed' && outcome !== 'failed') {
      throw new Error(`LessonStore.applyValidationOutcome: outcome must be 'passed' or 'failed'`);
    }

    const affectedLessonIds = [];
    let countedApplications = 0;
    const apply = this.db.transaction(() => {
      const applications = this.db
        .prepare('SELECT * FROM lesson_application WHERE run_id = ? AND counted = 0')
        .all(run_id);

      for (const application of applications) {
        const isPassed = outcome === 'passed';
        const counterColumn = isPassed ? 'helpful_count' : 'harmful_count';
        this._emitDelta({
          lesson_id: application.lesson_id,
          run_id,
          actor: 'validator-rule',
          delta_type: isPassed ? 'MARK_HELPFUL' : 'MARK_HARMFUL',
          payload: { application_id: application.application_id, outcome },
        });
        this.db
          .prepare(`UPDATE lesson SET ${counterColumn} = ${counterColumn} + 1 WHERE lesson_id = ?`)
          .run(application.lesson_id);
        this.db
          .prepare(
            'UPDATE lesson_application SET counted = 1, outcome = ? WHERE application_id = ?'
          )
          .run(outcome, application.application_id);
        if (!affectedLessonIds.includes(application.lesson_id)) {
          affectedLessonIds.push(application.lesson_id);
        }
        countedApplications++;
      }
    });
    apply();

    for (const lessonId of affectedLessonIds) {
      const lesson = this.getLesson(lessonId);
      if (lesson) {
        this.applyStatusRules(lesson, run_id);
      }
    }

    this.maybeCurate();

    return { run_id, outcome, updated: countedApplications, lessons: affectedLessonIds };
  }

  /**
   * §5.2 status rules (retention as inference). n = helpful + harmful, Wilson
   * z = 1.96. Promote candidate -> active when n >= 8 and wilson_lower > 0.5.
   * Quarantine when n >= 8 and wilson_upper < 0.45. Never hard-delete.
   */
  applyStatusRules(lesson, runId = null) {
    const n = lesson.helpful_count + lesson.harmful_count;
    if (n < STATUS_RULE_MIN_SAMPLES) {
      return lesson;
    }

    const { lower, upper } = wilsonInterval(lesson.helpful_count, n);

    if (lower > PROMOTION_WILSON_LOWER && lesson.status === 'candidate') {
      // DEVIATION 2 (see file header): promotion is applied to the lesson row
      // WITHOUT emitting a delta; replay (§6) recomputes it from counters.
      this.db
        .prepare("UPDATE lesson SET status = 'active' WHERE lesson_id = ?")
        .run(lesson.lesson_id);
      lesson.status = 'active';
    }

    if (upper < QUARANTINE_WILSON_UPPER && lesson.status !== 'quarantined') {
      this._emitDelta({
        lesson_id: lesson.lesson_id,
        run_id: runId,
        actor: 'validator-rule',
        delta_type: 'QUARANTINE',
        payload: {
          helpful_count: lesson.helpful_count,
          harmful_count: lesson.harmful_count,
          wilson_upper: upper,
        },
      });
      this.db
        .prepare("UPDATE lesson SET status = 'quarantined' WHERE lesson_id = ?")
        .run(lesson.lesson_id);
      lesson.status = 'quarantined';
    }

    return lesson;
  }

  /**
   * §7 curator pass. Acts only when at least `markInterval` MARK_* deltas sit
   * above the last_curation_delta_id watermark; then, in ONE transaction:
   * (a) merge candidate+active lessons sharing (failure_class, normalized
   *     trigger_cue) into the row with the highest helpful+harmful (counters
   *     and uses add, provenance unions, sources retire via MERGE_INTO delta),
   * (b) retire candidates with uses = 0 older than pruneDays via RETIRE delta,
   * (c) advance the watermark.
   *
   * The curator NEVER modifies explanation / intervention / trigger_cue text —
   * no re-summarization (ACE brevity-bias/context-collapse rule, §7).
   */
  maybeCurate({ markInterval = 25, pruneDays = 30 } = {}) {
    const watermark = Number(this._getMeta('last_curation_delta_id') || '0');
    const pendingMarks = this.db
      .prepare(
        `SELECT COUNT(*) AS n FROM lesson_delta
         WHERE delta_id > ? AND delta_type IN ('MARK_HELPFUL', 'MARK_HARMFUL')`
      )
      .get(watermark).n;

    if (pendingMarks < markInterval) {
      return { curated: false, merged: 0, pruned: 0 };
    }

    const curate = this.db.transaction(() => {
      const maxDeltaId =
        this.db.prepare('SELECT MAX(delta_id) AS maxId FROM lesson_delta').get().maxId ?? watermark;
      let merged = 0;
      let pruned = 0;

      // (a) MERGE exact-duplicate (failure_class, normalized trigger_cue) groups.
      const lessons = this.db
        .prepare("SELECT * FROM lesson WHERE status IN ('candidate', 'active')")
        .all();
      const groups = new Map();
      for (const lesson of lessons) {
        const key = `${lesson.failure_class} ${normalizeCue(lesson.trigger_cue)}`;
        if (!groups.has(key)) {
          groups.set(key, []);
        }
        groups.get(key).push(lesson);
      }

      for (const group of groups.values()) {
        if (group.length < 2) continue;
        // Absorber: highest (helpful + harmful); ties broken by oldest, then id.
        group.sort((a, b) => {
          const nA = a.helpful_count + a.harmful_count;
          const nB = b.helpful_count + b.harmful_count;
          if (nB !== nA) return nB - nA;
          if (a.created_at !== b.created_at) return a.created_at < b.created_at ? -1 : 1;
          return a.lesson_id < b.lesson_id ? -1 : 1;
        });
        const target = group[0];
        let helpful = target.helpful_count;
        let harmful = target.harmful_count;
        let uses = target.uses;
        const provenance = JSON.parse(target.provenance);

        for (const source of group.slice(1)) {
          this._emitDelta({
            lesson_id: source.lesson_id,
            run_id: null,
            actor: 'curator',
            delta_type: 'MERGE_INTO',
            // Moved amounts are recorded so replay can reconstruct both sides.
            payload: {
              target_lesson_id: target.lesson_id,
              helpful_count: source.helpful_count,
              harmful_count: source.harmful_count,
              uses: source.uses,
              provenance: JSON.parse(source.provenance),
            },
          });
          helpful += source.helpful_count;
          harmful += source.harmful_count;
          uses += source.uses;
          for (const runId of JSON.parse(source.provenance)) {
            unionProvenance(provenance, runId);
          }
          this.db
            .prepare(
              `UPDATE lesson
               SET helpful_count = 0, harmful_count = 0, uses = 0, status = 'retired'
               WHERE lesson_id = ?`
            )
            .run(source.lesson_id);
          merged++;
        }

        this.db
          .prepare(
            'UPDATE lesson SET helpful_count = ?, harmful_count = ?, uses = ?, provenance = ? WHERE lesson_id = ?'
          )
          .run(helpful, harmful, uses, JSON.stringify(provenance), target.lesson_id);
      }

      // (b) PRUNE stale unused candidates. Text fields are never touched.
      const cutoff = new Date(Date.now() - pruneDays * 24 * 60 * 60 * 1000).toISOString();
      const stale = this.db
        .prepare("SELECT * FROM lesson WHERE status = 'candidate' AND uses = 0 AND created_at < ?")
        .all(cutoff);
      for (const lesson of stale) {
        this._emitDelta({
          lesson_id: lesson.lesson_id,
          run_id: null,
          actor: 'curator',
          delta_type: 'RETIRE',
          payload: { reason: 'stale_candidate', prune_days: pruneDays },
        });
        this.db
          .prepare("UPDATE lesson SET status = 'retired' WHERE lesson_id = ?")
          .run(lesson.lesson_id);
        pruned++;
      }

      // (c) advance the curation watermark.
      this._setMeta('last_curation_delta_id', String(maxDeltaId));

      return { merged, pruned };
    });

    const { merged, pruned } = curate();
    return { curated: true, merged, pruned };
  }

  /**
   * Record an explicit behavior trace (ported from lyo-kernel recordTrace).
   * Traces are the evidence units preference pairs compare; they are plain
   * records, not lesson deltas. When trace_id is omitted it is derived
   * deterministically from content, identical inputs collapse to one row.
   */
  recordTrace({ trace_id, run_id = null, kind, summary, ref = null, payload }) {
    if (!kind || !summary) {
      throw new Error('LessonStore.recordTrace: kind and summary are required');
    }
    const traceId =
      trace_id ??
      `trace-${sha256(
        JSON.stringify({
          run_id: run_id ?? null,
          kind,
          summary,
          ref: ref ?? null,
          payload: payload ?? null,
        })
      ).slice(0, 24)}`;
    this.db
      .prepare(
        `INSERT INTO learning_trace (trace_id, run_id, kind, summary, ref, payload_json, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)`
      )
      .run(
        traceId,
        run_id ?? null,
        kind,
        summary,
        ref ?? null,
        payload === undefined ? null : JSON.stringify(payload),
        nowIso()
      );
    return this._ensureTrace(traceId);
  }

  getTrace(traceId) {
    return (
      this.db
        .prepare(
          `SELECT trace_id, run_id, kind, summary, ref, payload_json, created_at
           FROM learning_trace WHERE trace_id = ?`
        )
        .get(traceId) || null
    );
  }

  _ensureTrace(traceId) {
    const trace = this.getTrace(traceId);
    if (!trace) throw new Error(`unknown trace: ${traceId}`);
    return trace;
  }

  /**
   * Record a user preference of one trace over another (ported from
   * lyo-kernel recordPreferencePair). Guards mirror the kernel exactly:
   * distinct traces, an auditable reason, and both traces must exist.
   * context_hash defaults to a hash of the ordered pair so identical
   * comparisons share a context; preference_id is content-derived.
   */
  recordPreferencePair({
    chosen_trace_id,
    rejected_trace_id,
    reason,
    evidence_ref,
    confidence,
    recorded_by,
    context,
    context_hash,
    preference_id,
  }) {
    if (!chosen_trace_id || !rejected_trace_id || !reason || !evidence_ref) {
      throw new Error(
        'LessonStore.recordPreferencePair: chosen_trace_id, rejected_trace_id, reason, and evidence_ref are required'
      );
    }
    if (chosen_trace_id === rejected_trace_id) {
      throw new Error('preference pair requires distinct chosen and rejected traces');
    }
    if (reason.trim().length < 12) {
      throw new Error('preference reason must be specific enough to audit');
    }
    this._ensureTrace(chosen_trace_id);
    this._ensureTrace(rejected_trace_id);
    const contextHash =
      context_hash ?? sha256(context ?? `${chosen_trace_id}>${rejected_trace_id}`);
    const preferenceId =
      preference_id ??
      `pref-${contextHash.slice(0, 16)}-${sha256(`${chosen_trace_id}:${rejected_trace_id}:${reason}`).slice(0, 8)}`;
    this.db
      .prepare(
        `INSERT INTO preference_pair (
           preference_id, context_hash, chosen_trace_id, rejected_trace_id,
           reason, evidence_ref, recorded_by, confidence, created_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)`
      )
      .run(
        preferenceId,
        contextHash,
        chosen_trace_id,
        rejected_trace_id,
        reason,
        evidence_ref,
        recorded_by ?? null,
        confidence ?? 'medium',
        nowIso()
      );
    return this.getPreferencePair(preferenceId);
  }

  getPreferencePair(preferenceId) {
    return (
      this.db.prepare('SELECT * FROM preference_pair WHERE preference_id = ?').get(preferenceId) ||
      null
    );
  }

  /**
   * §6 replay: fold a lesson's deltas in delta_id order back into its state
   * (CREATE/EDIT payloads, MARK_* counts, status deltas; promotion recomputed
   * from counters per deviation 2). MERGE_INTO deltas targeting this lesson
   * are folded in as absorbed counters. `uses` is application-derived (not a
   * delta) and is therefore NOT part of the reconstructed state.
   */
  replayLesson(lessonId) {
    const own = this.getDeltas(lessonId);
    const absorbed = this.db
      .prepare("SELECT * FROM lesson_delta WHERE delta_type = 'MERGE_INTO' ORDER BY delta_id")
      .all()
      .filter((delta) => {
        try {
          return JSON.parse(delta.payload).target_lesson_id === lessonId;
        } catch {
          return false;
        }
      });
    const deltas = [...own, ...absorbed].sort((a, b) => a.delta_id - b.delta_id);

    let state = null;
    for (const delta of deltas) {
      const payload = JSON.parse(delta.payload);
      switch (delta.delta_type) {
        case 'CREATE':
          state = {
            lesson_id: lessonId,
            status: 'candidate',
            failure_class: payload.failure_class,
            trigger_cue: payload.trigger_cue,
            explanation: payload.explanation,
            intervention: payload.intervention,
            helpful_count: 0,
            harmful_count: 0,
            created_at: payload.created_at,
            updated_at: payload.updated_at,
            provenance: [...(payload.provenance || [])],
          };
          break;
        case 'EDIT':
          if (!state) break;
          unionProvenance(state.provenance, payload.run_id);
          if (payload.updated_at) state.updated_at = payload.updated_at;
          break;
        case 'MARK_HELPFUL':
          if (!state) break;
          state.helpful_count += 1;
          foldStatusRules(state);
          break;
        case 'MARK_HARMFUL':
          if (!state) break;
          state.harmful_count += 1;
          foldStatusRules(state);
          break;
        case 'QUARANTINE':
          if (state) state.status = 'quarantined';
          break;
        case 'RETIRE':
          if (state) state.status = 'retired';
          break;
        case 'REINSTATE':
          if (state) state.status = payload.to_status || 'candidate';
          break;
        case 'MERGE_INTO':
          if (!state) break;
          if (delta.lesson_id === lessonId) {
            // This lesson was merged away into a target.
            state.status = 'retired';
            state.helpful_count = 0;
            state.harmful_count = 0;
          } else {
            // This lesson absorbed the source's counters.
            state.helpful_count += payload.helpful_count || 0;
            state.harmful_count += payload.harmful_count || 0;
            for (const runId of payload.provenance || []) {
              unionProvenance(state.provenance, runId);
            }
          }
          break;
        default:
          break;
      }
    }

    return state;
  }
}

module.exports = LessonStore;
