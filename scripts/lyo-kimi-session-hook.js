#!/usr/bin/env node
/**
 * kimi-code SessionStart hook: inject LYO lessons into session context.
 *
 * On session start/resume, prints the top lessons from the local LYO lesson
 * store (zeroshot-lyo) to stdout; kimi-code appends stdout to the model's
 * context. This is the "delivery/eval" step of the learning loop at the
 * personal-CLI layer: lessons validated by past cluster runs show up where
 * future behavior happens.
 *
 * Selection reuses the cluster-side policy (thompson-beta@1 via
 * src/lyo/selection-policies.js): one Thompson draw per lesson over its
 * Beta(helpful+1, harmful+1) posterior, top N kept. Proven lessons usually
 * win; uncertain lessons occasionally surface — explore/exploit for free.
 *
 * Contracts:
 * - READ-ONLY. The hook never creates or writes a lesson DB (blast-radius
 *   containment mirrors src/lyo/observer.js Appendix B.4). Decision/propensity
 *   logging stays cluster-side; a personal-layer hook is not a writer.
 * - FAIL-OPEN. Any error (no DB, corrupt DB, bad payload) exits 0 silently;
 *   the session must never be blocked by a learning-layer hiccup.
 *
 * Install (~/.kimi-code/config.toml):
 *   [[hooks]]
 *   event = "SessionStart"
 *   command = "node /Users/marcus.kim/repositories/oss/zeroshot-lyo/scripts/lyo-kimi-session-hook.js"
 *   timeout = 5
 *
 * Store path resolution mirrors src/lyo/observer.js resolveLessonStorePath:
 * ZEROSHOT_LYO_STORE_PATH -> <session cwd>/.zeroshot/lyo-lessons.db ->
 * ~/.zeroshot/lyo-lessons.db (first existing file wins).
 */

const fs = require('fs');
const os = require('os');
const path = require('path');

const LIMIT = 5;
const MAX_TEXT = 220;
const OPEN_TIMEOUT_MS = 1000;
const STDIN_GUARD_MS = 2500;

function resolveStorePath(cwd) {
  const candidates = [];
  if (process.env.ZEROSHOT_LYO_STORE_PATH) {
    candidates.push(process.env.ZEROSHOT_LYO_STORE_PATH);
  }
  if (cwd) {
    candidates.push(path.join(cwd, '.zeroshot', 'lyo-lessons.db'));
  }
  candidates.push(path.join(os.homedir(), '.zeroshot', 'lyo-lessons.db'));
  for (const candidate of candidates) {
    try {
      if (fs.statSync(candidate).isFile()) {
        return candidate;
      }
    } catch {
      // not found / not readable — try next candidate
    }
  }
  return null;
}

function openReadOnly(dbPath) {
  const Database = require('better-sqlite3');
  try {
    return new Database(dbPath, { readonly: true, fileMustExist: true, timeout: OPEN_TIMEOUT_MS });
  } catch {
    // WAL recovery can refuse a readonly open; fall back to a normal open.
    // fileMustExist still guarantees we never create a DB.
    return new Database(dbPath, { fileMustExist: true, timeout: OPEN_TIMEOUT_MS });
  }
}

function formatLesson(row) {
  const text = String(row.intervention || '')
    .replace(/\s+/g, ' ')
    .trim();
  const short = text.length > MAX_TEXT ? `${text.slice(0, MAX_TEXT - 1)}…` : text;
  const mean = Number(row.posterior_mean || 0).toFixed(2);
  return `- [${mean} · ${row.helpful_count}✓/${row.harmful_count}✗ · ${row.failure_class}] ${short}`;
}

function main(payload) {
  const storePath = resolveStorePath(payload && payload.cwd);
  if (!storePath) {
    return;
  }
  const db = openReadOnly(storePath);
  try {
    const rows = db
      .prepare(
        `SELECT lesson_id, failure_class, trigger_cue, explanation, intervention,
                helpful_count, harmful_count, uses, posterior_mean
         FROM v_lesson_library`
      )
      .all();
    if (rows.length === 0) {
      return;
    }

    // Same selection policy as the cluster side (thompson-beta@1).
    const { resolvePolicy } = require('../src/lyo/selection-policies');
    const policy = resolvePolicy(null);
    const candidates = rows.map((row) => ({
      lesson_id: row.lesson_id,
      alpha: row.helpful_count + 1,
      beta: row.harmful_count + 1,
    }));
    const picks = policy.sampleSelection(candidates, LIMIT);
    const lines = picks.map(({ index }) => formatLesson(rows[index]));

    process.stdout.write(
      `LYO lessons from your past runs (${rows.length} in library, Thompson-sampled top ${picks.length}; ` +
        `score = posterior mean, ✓/✗ = validated outcomes; apply when relevant):\n` +
        `${lines.join('\n')}\n`
    );
  } finally {
    db.close();
  }
}

let input = '';
process.stdin.on('data', (chunk) => {
  input += chunk;
});
process.stdin.on('end', () => {
  try {
    main(JSON.parse(input || '{}'));
  } catch {
    // fail-open: never block the session
  }
  process.exit(0);
});
// Safety: if stdin never closes, still exit cleanly within the hook timeout.
setTimeout(() => process.exit(0), STDIN_GUARD_MS).unref();
