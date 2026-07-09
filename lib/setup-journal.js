/**
 * Undo journal for `zeroshot setup apply` / `zeroshot setup undo`.
 *
 * One journal entry per setting this setup owns: { scope, path, repoRoot,
 * priorValue, appliedValue, appliedAt }. `priorValue: null` means the key
 * did not exist before apply, so undo deletes it rather than restoring it.
 * Shared by lib/setup-apply.js (writer) and lib/setup-undo.js (reader) so the
 * nested-path mutation and equality semantics can't drift between the two.
 */

const fs = require('fs');
const path = require('path');
const { getSettingsFile } = require('./settings');
const { getNestedValue } = require('./setup-plan');

function getJournalPath() {
  return path.join(path.dirname(getSettingsFile()), 'setup-undo-journal.json');
}

function loadJournal() {
  const journalPath = getJournalPath();
  if (!fs.existsSync(journalPath)) {
    return { version: 1, entries: [] };
  }
  try {
    const parsed = JSON.parse(fs.readFileSync(journalPath, 'utf8'));
    if (!parsed || !Array.isArray(parsed.entries)) {
      return { version: 1, entries: [] };
    }
    return parsed;
  } catch {
    return { version: 1, entries: [] };
  }
}

function saveJournal(journal) {
  const journalPath = getJournalPath();
  fs.mkdirSync(path.dirname(journalPath), { recursive: true });
  fs.writeFileSync(journalPath, JSON.stringify(journal, null, 2), 'utf8');
}

function entryKey(entry) {
  return `${entry.scope}:${entry.repoRoot || ''}:${entry.path}`;
}

// Re-applying an already-journaled write updates appliedValue/appliedAt but
// keeps the original priorValue — that's the true pre-apply state undo must
// restore to, and it must not drift across repeated `apply` runs.
function upsertJournalEntry(journal, entry) {
  const key = entryKey(entry);
  const existingIndex = journal.entries.findIndex((e) => entryKey(e) === key);
  if (existingIndex === -1) {
    journal.entries.push(entry);
    return;
  }
  journal.entries[existingIndex] = {
    ...entry,
    priorValue: journal.entries[existingIndex].priorValue,
  };
}

function setNestedValue(target, pathStr, value) {
  const keys = pathStr.split('.');
  let node = target;
  for (let i = 0; i < keys.length - 1; i++) {
    const key = keys[i];
    if (typeof node[key] !== 'object' || node[key] === null) {
      node[key] = {};
    }
    node = node[key];
  }
  node[keys[keys.length - 1]] = value;
}

function deleteNestedKey(target, pathStr) {
  const keys = pathStr.split('.');
  let node = target;
  for (let i = 0; i < keys.length - 1; i++) {
    const key = keys[i];
    if (typeof node[key] !== 'object' || node[key] === null) return;
    node = node[key];
  }
  delete node[keys[keys.length - 1]];
}

function deepEqual(a, b) {
  if (a === b) return true;
  if (a === null || b === null || typeof a !== 'object' || typeof b !== 'object') return false;
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  if (Array.isArray(a)) {
    if (a.length !== b.length) return false;
    return a.every((v, i) => deepEqual(v, b[i]));
  }
  const aKeys = Object.keys(a);
  const bKeys = Object.keys(b);
  if (aKeys.length !== bKeys.length) return false;
  return aKeys.every((k) => Object.prototype.hasOwnProperty.call(b, k) && deepEqual(a[k], b[k]));
}

module.exports = {
  getJournalPath,
  loadJournal,
  saveJournal,
  upsertJournalEntry,
  getNestedValue,
  setNestedValue,
  deleteNestedKey,
  deepEqual,
};
