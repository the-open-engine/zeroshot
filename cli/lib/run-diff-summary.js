const fs = require('fs');
const os = require('os');
const path = require('path');
const { execFileSync } = require('child_process');

function defaultStorageDir() {
  const homeDir =
    process.env.ZEROSHOT_HOME || process.env.HOME || process.env.USERPROFILE || os.homedir();
  return path.join(homeDir, '.zeroshot');
}

function runGit(repoPath, args) {
  try {
    return execFileSync('git', ['-C', repoPath, ...args], {
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'ignore'],
    }).trim();
  } catch {
    return null;
  }
}

function isGitRepo(repoPath) {
  return Boolean(repoPath) && Boolean(runGit(repoPath, ['rev-parse', '--show-toplevel']));
}

function listLocalBranches(repoRoot) {
  if (!isGitRepo(repoRoot)) {
    return new Set();
  }
  const output = runGit(repoRoot, ['for-each-ref', '--format=%(refname:short)', 'refs/heads']);
  if (!output) {
    return new Set();
  }
  return new Set(
    output
      .split('\n')
      .map((line) => line.trim())
      .filter(Boolean)
  );
}

function resolveRunGitContext(run, { storageDir, repoRoot, branchNames }) {
  const worktreePath = path.join(storageDir, 'worktrees', run.id);
  if (fs.existsSync(worktreePath) && isGitRepo(worktreePath)) {
    return {
      repoPath: worktreePath,
      headRef: 'HEAD',
      branchName: runGit(worktreePath, ['branch', '--show-current']) || 'HEAD',
      source: 'worktree',
    };
  }

  const branchName = `zeroshot/${run.id}`;
  if (!repoRoot || !branchNames.has(branchName)) {
    return null;
  }

  return {
    repoPath: repoRoot,
    headRef: branchName,
    branchName,
    source: 'branch',
  };
}

function resolveBaseRef(repoPath, headRef) {
  const candidates = ['dev', 'origin/dev', 'main', 'origin/main'];
  for (const candidate of candidates) {
    if (!runGit(repoPath, ['rev-parse', '--quiet', '--verify', candidate])) {
      continue;
    }
    const mergeBase = runGit(repoPath, ['merge-base', headRef, candidate]);
    if (mergeBase) {
      return { baseRef: candidate, mergeBase };
    }
  }
  return null;
}

function parseShortStat(shortStat) {
  const text = typeof shortStat === 'string' ? shortStat : '';
  const filesChanged = Number.parseInt(text.match(/(\d+)\s+files?\s+changed/)?.[1] || '0', 10);
  const insertions = Number.parseInt(text.match(/(\d+)\s+insertions?\(\+\)/)?.[1] || '0', 10);
  const deletions = Number.parseInt(text.match(/(\d+)\s+deletions?\(-\)/)?.[1] || '0', 10);

  return {
    filesChanged,
    insertions,
    deletions,
  };
}

function formatRunDiffSummary(diff) {
  if (!diff || diff.available !== true) {
    return '-';
  }
  return `${diff.commits}c ${diff.filesChanged}f +${diff.insertions}/-${diff.deletions}`;
}

function summarizeRunDiff(run, options = {}) {
  const storageDir = options.storageDir || defaultStorageDir();
  const repoRoot = options.repoRoot || null;
  const branchNames = options.branchNames || new Set();

  const gitContext = resolveRunGitContext(run, { storageDir, repoRoot, branchNames });
  if (!gitContext) {
    return {
      available: false,
      reason: 'no_git_context',
    };
  }

  const base = resolveBaseRef(gitContext.repoPath, gitContext.headRef);
  if (!base) {
    return {
      available: false,
      reason: 'no_base_ref',
      branchName: gitContext.branchName,
      source: gitContext.source,
    };
  }

  const shortStat = runGit(gitContext.repoPath, [
    'diff',
    '--shortstat',
    `${base.mergeBase}..${gitContext.headRef}`,
  ]);
  const counts = parseShortStat(shortStat);
  const commitCountText = runGit(gitContext.repoPath, [
    'rev-list',
    '--count',
    `${base.mergeBase}..${gitContext.headRef}`,
  ]);
  const commits = Number.parseInt(commitCountText || '0', 10);
  const head = runGit(gitContext.repoPath, ['log', '-1', '--format=%h %s', gitContext.headRef]) || null;

  return {
    available: true,
    source: gitContext.source,
    repoPath: gitContext.repoPath,
    branchName: gitContext.branchName,
    headRef: gitContext.headRef,
    baseRef: base.baseRef,
    mergeBase: base.mergeBase,
    commits: Number.isFinite(commits) ? commits : 0,
    head,
    ...counts,
  };
}

function enrichRunsWithDiff(runs, options = {}) {
  const storageDir = options.storageDir || defaultStorageDir();
  const repoRoot = options.repoRoot || null;
  const branchNames = options.branchNames || listLocalBranches(repoRoot);

  return runs.map((run) => {
    const diff = summarizeRunDiff(run, { storageDir, repoRoot, branchNames });
    return {
      ...run,
      diff,
      diffSummary: formatRunDiffSummary(diff),
    };
  });
}

module.exports = {
  defaultStorageDir,
  enrichRunsWithDiff,
  formatRunDiffSummary,
  listLocalBranches,
  parseShortStat,
  summarizeRunDiff,
};
