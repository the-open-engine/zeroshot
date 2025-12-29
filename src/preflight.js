/**
 * Preflight Validation - Check all dependencies before starting
 *
 * Validates:
 * - Claude CLI installed and authenticated
 * - gh CLI installed and authenticated (if using issue numbers)
 * - Docker available (if using --docker)
 *
 * Provides CLEAR, ACTIONABLE error messages with recovery instructions.
 */

const { execSync } = require('child_process');
const path = require('path');
const fs = require('fs');
const os = require('os');

/**
 * Validation result
 * @typedef {Object} ValidationResult
 * @property {boolean} valid - Whether validation passed
 * @property {string[]} errors - Fatal errors that block execution
 * @property {string[]} warnings - Non-fatal warnings
 */

/**
 * Format error with recovery instructions
 * @param {string} title - Error title
 * @param {string} detail - Error details
 * @param {string[]} recovery - Recovery steps
 * @returns {string}
 */
function formatError(title, detail, recovery) {
  let msg = `\n❌ ${title}\n`;
  msg += `   ${detail}\n`;
  if (recovery.length > 0) {
    msg += `\n   To fix:\n`;
    recovery.forEach((step, i) => {
      msg += `   ${i + 1}. ${step}\n`;
    });
  }
  return msg;
}

/**
 * Check if a command exists
 * @param {string} cmd - Command to check
 * @returns {boolean}
 */
function commandExists(cmd) {
  try {
    execSync(`which ${cmd}`, { encoding: 'utf8', stdio: 'pipe' });
    return true;
  } catch {
    return false;
  }
}

/**
 * Get Claude CLI version
 * @returns {{ installed: boolean, version: string | null, error: string | null }}
 */
function getClaudeVersion() {
  try {
    const output = execSync('claude --version', { encoding: 'utf8', stdio: 'pipe' });
    const match = output.match(/(\d+\.\d+\.\d+)/);
    return {
      installed: true,
      version: match ? match[1] : 'unknown',
      error: null,
    };
  } catch (err) {
    if (err.message.includes('command not found') || err.message.includes('not found')) {
      return {
        installed: false,
        version: null,
        error: 'Claude CLI not installed',
      };
    }
    return {
      installed: false,
      version: null,
      error: err.message,
    };
  }
}

/**
 * Check Claude CLI authentication status
 * @returns {{ authenticated: boolean, error: string | null, configDir: string }}
 */
function checkClaudeAuth() {
  const configDir = process.env.CLAUDE_CONFIG_DIR || path.join(os.homedir(), '.claude');
  const credentialsPath = path.join(configDir, '.credentials.json');

  // Check if credentials file exists
  if (!fs.existsSync(credentialsPath)) {
    return {
      authenticated: false,
      error: 'No credentials file found',
      configDir,
    };
  }

  // Check if credentials file has content
  try {
    const content = fs.readFileSync(credentialsPath, 'utf8');
    const creds = JSON.parse(content);

    // Check for OAuth token (primary auth method)
    if (creds.claudeAiOauth?.accessToken) {
      // Check if token is expired
      const expiresAt = creds.claudeAiOauth.expiresAt;
      if (expiresAt && new Date(expiresAt) < new Date()) {
        return {
          authenticated: false,
          error: 'OAuth token expired',
          configDir,
        };
      }
      return {
        authenticated: true,
        error: null,
        configDir,
      };
    }

    // Check for API key auth
    if (creds.apiKey) {
      return {
        authenticated: true,
        error: null,
        configDir,
      };
    }

    return {
      authenticated: false,
      error: 'No valid authentication found in credentials',
      configDir,
    };
  } catch (err) {
    return {
      authenticated: false,
      error: `Failed to parse credentials: ${err.message}`,
      configDir,
    };
  }
}

/**
 * Check gh CLI authentication status
 * @returns {{ installed: boolean, authenticated: boolean, error: string | null }}
 */
function checkGhAuth() {
  // Check if gh is installed
  if (!commandExists('gh')) {
    return {
      installed: false,
      authenticated: false,
      error: 'gh CLI not installed',
    };
  }

  // Check auth status
  try {
    execSync('gh auth status', { encoding: 'utf8', stdio: 'pipe' });
    return {
      installed: true,
      authenticated: true,
      error: null,
    };
  } catch (err) {
    // gh auth status returns non-zero if not authenticated
    const stderr = err.stderr || err.message || '';

    if (stderr.includes('not logged in')) {
      return {
        installed: true,
        authenticated: false,
        error: 'gh CLI not authenticated',
      };
    }

    return {
      installed: true,
      authenticated: false,
      error: stderr.trim() || 'Unknown gh auth error',
    };
  }
}

/**
 * Check Docker availability
 * @returns {{ available: boolean, error: string | null }}
 */
function checkDocker() {
  try {
    execSync('docker --version', { encoding: 'utf8', stdio: 'pipe' });

    // Also check if Docker daemon is running
    execSync('docker info', { encoding: 'utf8', stdio: 'pipe' });

    return {
      available: true,
      error: null,
    };
  } catch (err) {
    const stderr = err.stderr || err.message || '';

    if (stderr.includes('command not found') || stderr.includes('not found')) {
      return {
        available: false,
        error: 'Docker not installed',
      };
    }

    if (stderr.includes('Cannot connect') || stderr.includes('Is the docker daemon running')) {
      return {
        available: false,
        error: 'Docker daemon not running',
      };
    }

    return {
      available: false,
      error: stderr.trim() || 'Unknown Docker error',
    };
  }
}

/**
 * Run all preflight checks
 * @param {Object} options - Preflight options
 * @param {boolean} options.requireGh - Whether gh CLI is required (true if using issue number)
 * @param {boolean} options.requireDocker - Whether Docker is required (true if using --docker)
 * @param {boolean} options.requireGit - Whether git repo is required (true if using --worktree)
 * @param {boolean} options.quiet - Suppress success messages
 * @returns {ValidationResult}
 */
function runPreflight(options = {}) {
  const errors = [];
  const warnings = [];

  // 1. Check Claude CLI installation
  const claude = getClaudeVersion();
  if (!claude.installed) {
    errors.push(
      formatError(
        'Claude CLI not installed',
        claude.error,
        [
          'Install Claude CLI: npm install -g @anthropic-ai/claude-code',
          'Or: brew install claude (macOS)',
          'Then run: claude --version',
        ]
      )
    );
  } else {
    // 2. Check Claude CLI authentication
    const auth = checkClaudeAuth();
    if (!auth.authenticated) {
      errors.push(
        formatError(
          'Claude CLI not authenticated',
          auth.error,
          [
            'Run: claude login',
            'Follow the browser prompts to authenticate',
            `Config directory: ${auth.configDir}`,
          ]
        )
      );
    }

    // Check version (warn if old)
    if (claude.version) {
      const [major, minor] = claude.version.split('.').map(Number);
      if (major < 1 || (major === 1 && minor < 0)) {
        warnings.push(
          `⚠️  Claude CLI version ${claude.version} may be outdated. Consider upgrading.`
        );
      }
    }
  }

  // 3. Check gh CLI (if required)
  if (options.requireGh) {
    const gh = checkGhAuth();
    if (!gh.installed) {
      errors.push(
        formatError(
          'GitHub CLI (gh) not installed',
          'Required for fetching issues by number',
          [
            'Install: brew install gh (macOS) or apt install gh (Linux)',
            'Or download from: https://cli.github.com/',
          ]
        )
      );
    } else if (!gh.authenticated) {
      errors.push(
        formatError(
          'GitHub CLI (gh) not authenticated',
          gh.error,
          [
            'Run: gh auth login',
            'Select GitHub.com, HTTPS, and authenticate via browser',
            'Then verify: gh auth status',
          ]
        )
      );
    }
  }

  // 4. Check Docker (if required)
  if (options.requireDocker) {
    const docker = checkDocker();
    if (!docker.available) {
      errors.push(
        formatError(
          'Docker not available',
          docker.error,
          docker.error.includes('daemon')
            ? ['Start Docker Desktop', 'Or run: sudo systemctl start docker (Linux)']
            : [
                'Install Docker Desktop from: https://docker.com/products/docker-desktop',
                'Then start Docker and verify: docker info',
              ]
        )
      );
    }
  }

  // 5. Check git repo (if required for worktree isolation)
  if (options.requireGit) {
    let isGitRepo = false;
    try {
      execSync('git rev-parse --git-dir', { stdio: 'pipe' });
      isGitRepo = true;
    } catch {
      // Not a git repo
    }
    if (!isGitRepo) {
      errors.push(
        formatError(
          'Not in a git repository',
          'Worktree isolation requires a git repository',
          [
            'Run from within a git repository',
            'Or use --docker instead of --worktree for non-git directories',
            'Initialize a repo with: git init',
          ]
        )
      );
    }
  }

  return {
    valid: errors.length === 0,
    errors,
    warnings,
  };
}

/**
 * Run preflight checks and exit if failed
 * @param {Object} options - Preflight options
 * @param {boolean} options.requireGh - Whether gh CLI is required
 * @param {boolean} options.requireDocker - Whether Docker is required
 * @param {boolean} options.requireGit - Whether git repo is required
 * @param {boolean} options.quiet - Suppress success messages
 */
function requirePreflight(options = {}) {
  const result = runPreflight(options);

  // Print warnings regardless of success
  if (result.warnings.length > 0) {
    for (const warning of result.warnings) {
      console.warn(warning);
    }
  }

  if (!result.valid) {
    console.error('\n' + '='.repeat(60));
    console.error('PREFLIGHT CHECK FAILED');
    console.error('='.repeat(60));

    for (const error of result.errors) {
      console.error(error);
    }

    console.error('='.repeat(60) + '\n');
    process.exit(1);
  }

  if (!options.quiet) {
    console.log('✓ Preflight checks passed');
  }
}

module.exports = {
  runPreflight,
  requirePreflight,
  getClaudeVersion,
  checkClaudeAuth,
  checkGhAuth,
  checkDocker,
  formatError,
};
