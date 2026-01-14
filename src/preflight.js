/**
 * Preflight Validation - Check all dependencies before starting
 *
 * Validates:
 * - Selected provider CLI installed
 * - gh CLI installed and authenticated (if using issue numbers)
 * - Docker available (if using --docker)
 *
 * Provides CLEAR, ACTIONABLE error messages with recovery instructions.
 */

const { execSync } = require('./lib/safe-exec'); // Enforces timeouts
const path = require('path');
const fs = require('fs');
const os = require('os');
const {
  isValidAnthropicKey,
  ANTHROPIC_KEY_PREFIX,
  resolveClaudeAuth,
} = require('../lib/settings/claude-auth.js');
const { loadSettings, getClaudeCommand } = require('../lib/settings.js');
const { normalizeProviderName } = require('../lib/provider-names');

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
 * Check if a command exists (cross-platform)
 * @param {string} cmd - Command to check
 * @returns {boolean}
 */
function commandExists(cmd) {
  try {
    // Windows uses 'where', Unix uses 'which'
    const checkCmd = process.platform === 'win32' ? `where ${cmd}` : `which ${cmd}`;
    execSync(checkCmd, { encoding: 'utf8', stdio: 'pipe' });
    return true;
  } catch {
    return false;
  }
}

/**
 * Get Claude CLI version
 * @param {string} claudeCommand - Optional custom Claude command (e.g., 'ccr code')
 * @returns {{ installed: boolean, version: string | null, error: string | null }}
 */
function getClaudeVersion(claudeCommand = 'claude') {
  // Parse command parts
  const parts = claudeCommand.trim().split(/\s+/);
  const command = parts[0];
  const extraArgs = parts.slice(1);

  try {
    const versionArgs = [...extraArgs, '--version'];
    const versionCmd = [command, ...versionArgs].join(' ');
    const output = execSync(versionCmd, { encoding: 'utf8', stdio: 'pipe' });
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
        error: `Command '${command}' not installed`,
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
 * Check macOS Keychain for Claude Code credentials
 * @returns {{ authenticated: boolean, error: string | null }}
 */
function checkMacOsKeychain() {
  if (os.platform() !== 'darwin') {
    return { authenticated: false, error: 'Not macOS' };
  }

  try {
    // Check if Claude Code credentials exist in Keychain
    execSync('security find-generic-password -s "Claude Code-credentials"', {
      encoding: 'utf8',
      stdio: 'pipe',
      timeout: 2000,
    });
    return { authenticated: true, error: null };
  } catch {
    return { authenticated: false, error: 'No credentials in Keychain' };
  }
}

/**
 * Check Claude CLI authentication status
 * @returns {{ authenticated: boolean, error: string | null, configDir: string, method?: string }}
 */
function checkClaudeAuth() {
  const configDir = process.env.CLAUDE_CONFIG_DIR || path.join(os.homedir(), '.claude');

  // Helper to create consistent result objects
  const authResult = (authenticated, error = null, method = null) => ({
    authenticated,
    error,
    configDir,
    ...(method && { method }),
  });

  // Check for Bedrock bearer token (highest priority)
  if (process.env.AWS_BEARER_TOKEN_BEDROCK) {
    if (!process.env.AWS_REGION) {
      return authResult(false, 'AWS_BEARER_TOKEN_BEDROCK set but AWS_REGION is missing');
    }
    return authResult(true, null, 'bedrock_api_key');
  }

  // Check for ANTHROPIC_API_KEY environment variable
  const apiKeyEnv = process.env.ANTHROPIC_API_KEY;
  if (apiKeyEnv) {
    if (!isValidAnthropicKey(apiKeyEnv)) {
      return authResult(false, `ANTHROPIC_API_KEY must start with ${ANTHROPIC_KEY_PREFIX}`);
    }
    return authResult(true, null, 'env_api_key');
  }

  // Check for settings-based auth (anthropicApiKey or bedrockApiKey in settings)
  const settings = loadSettings();
  const settingsAuth = resolveClaudeAuth(settings);
  if (settingsAuth.ANTHROPIC_API_KEY) {
    return authResult(true, null, 'settings_api_key');
  }
  if (settingsAuth.AWS_BEARER_TOKEN_BEDROCK) {
    if (!settingsAuth.AWS_REGION && !process.env.AWS_REGION) {
      return authResult(false, 'Bedrock configured in settings but AWS_REGION is missing');
    }
    return authResult(true, null, 'settings_bedrock');
  }

  const credentialsPath = path.join(configDir, '.credentials.json');

  // Check if credentials file exists
  if (!fs.existsSync(credentialsPath)) {
    // No credentials file - check macOS Keychain as fallback
    // Only use Keychain when using default config dir (not custom CLAUDE_CONFIG_DIR)
    if (!process.env.CLAUDE_CONFIG_DIR && checkMacOsKeychain().authenticated) {
      return authResult(true, null, 'keychain');
    }
    return authResult(false, 'No credentials file found');
  }

  // Check if credentials file has content
  try {
    const content = fs.readFileSync(credentialsPath, 'utf8');
    const creds = JSON.parse(content);

    // Check for OAuth token (primary auth method)
    if (creds.claudeAiOauth?.accessToken) {
      const expiresAt = creds.claudeAiOauth.expiresAt;
      if (expiresAt && new Date(expiresAt) < new Date()) {
        return authResult(false, 'OAuth token expired');
      }
      return authResult(true);
    }

    // Check for API key auth
    if (creds.apiKey) {
      return authResult(true);
    }

    return authResult(false, 'No valid authentication found in credentials');
  } catch (err) {
    return authResult(false, `Failed to parse credentials: ${err.message}`);
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
 * @param {string} options.claudeCommand - Custom Claude command (from settings)
 * @param {string} options.provider - Provider override
 * @returns {ValidationResult}
 */
function runPreflight(options = {}) {
  const errors = [];
  const warnings = [];

  const settings = loadSettings();
  const providerName = normalizeProviderName(
    options.provider || settings.defaultProvider || 'claude'
  );

  if (providerName === 'claude') {
    const { command, args } = getClaudeCommand();
    const claudeCommand = options.claudeCommand || [command, ...args].join(' ');

    const claude = getClaudeVersion(claudeCommand);
    if (!claude.installed) {
      errors.push(
        formatError(
          'Claude command not available',
          claude.error,
          claudeCommand === 'claude'
            ? [
                'Install Claude CLI: npm install -g @anthropic-ai/claude-code',
                'Or: brew install claude (macOS)',
                'Then run: claude --version',
              ]
            : [
                `Command '${claudeCommand}' not found`,
                'Check settings: zeroshot settings',
                'Update claudeCommand: zeroshot settings set claudeCommand "your-command"',
                'Or install the missing command',
              ]
        )
      );
    } else if (claude.version) {
      const [major, minor] = claude.version.split('.').map(Number);
      if (major < 1 || (major === 1 && minor < 0)) {
        warnings.push(
          `⚠️  Claude CLI version ${claude.version} may be outdated. Consider upgrading.`
        );
      }
    }

    // Claude CLI refuses --dangerously-skip-permissions when running as root.
    if (process.getuid && process.getuid() === 0) {
      errors.push(
        formatError(
          'Running as root',
          'Claude CLI refuses --dangerously-skip-permissions flag when running as root (UID 0)',
          [
            'Run as non-root user in Docker: docker run --user 1000:1000 ...',
            'Or create non-root user: adduser testuser && su - testuser',
            'Or use existing node user: docker run --user node ...',
            'Security: Claude CLI blocks this flag as root to prevent privilege escalation',
          ]
        )
      );
    }
  } else if (providerName === 'codex') {
    if (!commandExists('codex')) {
      errors.push(
        formatError('Codex CLI not available', 'Command "codex" not installed', [
          'Install Codex CLI: npm install -g @openai/codex',
          'Then run: codex --version',
        ])
      );
    }
  } else if (providerName === 'gemini') {
    if (!commandExists('gemini')) {
      errors.push(
        formatError('Gemini CLI not available', 'Command "gemini" not installed', [
          'Install Gemini CLI: npm install -g @google/gemini-cli',
          'Then run: gemini --version',
        ])
      );
    }
  } else if (providerName === 'opencode') {
    if (!commandExists('opencode')) {
      errors.push(
        formatError('Opencode CLI not available', 'Command "opencode" not installed', [
          'Install Opencode CLI: see https://opencode.ai',
          'Then run: opencode --version',
        ])
      );
    }
  } else {
    errors.push(
      formatError('Unknown provider', `Provider "${providerName}" is not supported`, [
        'Use claude, codex, gemini, or opencode',
      ])
    );
  }

  // 4. Check gh CLI (if required)
  if (options.requireGh) {
    const gh = checkGhAuth();
    if (!gh.installed) {
      errors.push(
        formatError('GitHub CLI (gh) not installed', 'Required for fetching issues by number', [
          'Install: brew install gh (macOS) or apt install gh (Linux)',
          'Or download from: https://cli.github.com/',
        ])
      );
    } else if (!gh.authenticated) {
      errors.push(
        formatError('GitHub CLI (gh) not authenticated', gh.error, [
          'Run: gh auth login',
          'Select GitHub.com, HTTPS, and authenticate via browser',
          'Then verify: gh auth status',
        ])
      );
    }
  }

  // 5. Check Docker (if required)
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

  // 6. Check git repo (if required for worktree isolation)
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
        formatError('Not in a git repository', 'Worktree isolation requires a git repository', [
          'Run from within a git repository',
          'Or use --docker instead of --worktree for non-git directories',
          'Initialize a repo with: git init',
        ])
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
 * @param {string} options.provider - Provider override
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
