/**
 * Docker Compose teardown resolution for worktree cleanup.
 *
 * Zeroshot never runs `docker compose up` — any compose file found in a worktree
 * belongs to the target repo, not to zeroshot. Tearing it down is only safe when
 * the resolved Compose project name is scoped to the worktree directory itself
 * (i.e. nothing pins it to the host's real, possibly-already-running project).
 */

const fs = require('fs');
const path = require('path');

const COMPOSE_FILENAMES = [
  'docker-compose.yml',
  'docker-compose.yaml',
  'compose.yml',
  'compose.yaml',
];

/**
 * Reproduce Docker Compose's default project-name normalization
 * (compose-go NormalizeProjectName: lowercase -> keep [a-z0-9_-] -> trim leading '_'/'-').
 *
 * @param {string} name
 * @returns {string}
 */
function normalizeComposeProjectName(name) {
  return name
    .toLowerCase()
    .replace(/[^a-z0-9_-]/g, '')
    .replace(/^[-_]+/, '');
}

/**
 * Decide whether it's safe to run `docker compose down` in a worktree, and with what args.
 * Never includes `--volumes` — automatic cleanup must not delete named volumes.
 * Skips teardown entirely when the Compose project name is pinned (top-level `name:`
 * in the compose file, or `COMPOSE_PROJECT_NAME` env var), since that resolves to a
 * shared/host project zeroshot did not create.
 *
 * @param {string} worktreePath - Path to the worktree directory
 * @returns {{ shouldTeardown: boolean, reason?: string, composePath?: string, args?: string[] }}
 */
function resolveWorktreeComposeTeardown(worktreePath) {
  let composePath = null;
  for (const filename of COMPOSE_FILENAMES) {
    const candidate = path.join(worktreePath, filename);
    if (fs.existsSync(candidate)) {
      composePath = candidate;
      break;
    }
  }

  if (!composePath) {
    return { shouldTeardown: false, reason: 'no compose file' };
  }

  if (
    typeof process.env.COMPOSE_PROJECT_NAME === 'string' &&
    process.env.COMPOSE_PROJECT_NAME.trim() !== ''
  ) {
    return {
      shouldTeardown: false,
      reason: 'pinned compose project name (shared host project)',
      composePath,
    };
  }

  try {
    const yaml = require('js-yaml');
    const contents = fs.readFileSync(composePath, 'utf8');
    const parsed = yaml.load(contents);
    if (parsed && typeof parsed === 'object' && parsed.name) {
      return {
        shouldTeardown: false,
        reason: 'pinned compose project name (shared host project)',
        composePath,
      };
    }
  } catch {
    // Fail safe: if the compose file can't be read/parsed, we cannot confirm project
    // identity is worktree-scoped, so never tear down.
    return { shouldTeardown: false, reason: 'compose file unreadable or unparsable', composePath };
  }

  return {
    shouldTeardown: true,
    composePath,
    args: [
      'compose',
      '-p',
      normalizeComposeProjectName(path.basename(worktreePath)),
      'down',
      '--remove-orphans',
      '--timeout',
      '10',
    ],
  };
}

module.exports = { resolveWorktreeComposeTeardown, normalizeComposeProjectName };
