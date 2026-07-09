/**
 * `zeroshot setup undo` — reverts writes made by `zeroshot setup apply` using
 * the journal recorded at apply time (lib/setup-journal.js).
 *
 * Three-way conflict rule per journaled write, comparing `current` against
 * `appliedValue`:
 *   - current === appliedValue      -> restore priorValue (delete if null)
 *   - current === priorValue        -> already-restored (no-op)
 *   - otherwise (changed elsewhere) -> skipped-modified, never clobbered
 */

const {
  loadJournal,
  getNestedValue,
  setNestedValue,
  deleteNestedKey,
  deepEqual,
} = require('./setup-journal');

function defaultUndoDeps() {
  const { loadSettings, saveSettings } = require('./settings');
  const { readRepoSettings, writeRepoSettings } = require('./repo-settings');

  return { loadSettings, saveSettings, readRepoSettings, writeRepoSettings };
}

/**
 * Undo every journaled write, per the three-way conflict rule.
 *
 * @param {Object} [params]
 * @param {Object} [params.deps] - Injected dependencies (for testing).
 * @returns {Array<{scope: string, path: string, repoRoot: string|null, status: string, current?: *, wouldRestore?: *}>}
 */
function undo({ deps = {} } = {}) {
  const resolvedDeps = { ...defaultUndoDeps(), ...deps };
  const journal = loadJournal();

  const globalSettings = resolvedDeps.loadSettings();
  const repoSettingsCache = new Map();
  let globalDirty = false;
  const dirtyRepoRoots = new Set();

  function repoSettingsFor(repoRoot) {
    if (!repoSettingsCache.has(repoRoot)) {
      const { settings } = resolvedDeps.readRepoSettings(repoRoot);
      repoSettingsCache.set(repoRoot, settings || {});
    }
    return repoSettingsCache.get(repoRoot);
  }

  const results = journal.entries.map((entry) => {
    const settingsObj = entry.scope === 'repo' ? repoSettingsFor(entry.repoRoot) : globalSettings;
    const current = getNestedValue(settingsObj, entry.path) ?? null;

    if (deepEqual(current, entry.priorValue)) {
      return { ...entry, status: 'already-restored' };
    }

    if (!deepEqual(current, entry.appliedValue)) {
      return { ...entry, status: 'skipped-modified', current, wouldRestore: entry.priorValue };
    }

    if (entry.priorValue === null) {
      deleteNestedKey(settingsObj, entry.path);
    } else {
      setNestedValue(settingsObj, entry.path, entry.priorValue);
    }

    if (entry.scope === 'repo') {
      dirtyRepoRoots.add(entry.repoRoot);
    } else {
      globalDirty = true;
    }

    return { ...entry, status: entry.priorValue === null ? 'deleted' : 'restored' };
  });

  if (globalDirty) resolvedDeps.saveSettings(globalSettings);
  for (const repoRoot of dirtyRepoRoots) {
    resolvedDeps.writeRepoSettings(repoRoot, repoSettingsFor(repoRoot));
  }

  // Journal entries are left in place (not cleared) so a re-run reports
  // 'already-restored' instead of finding nothing to undo.
  return results;
}

module.exports = { undo };
