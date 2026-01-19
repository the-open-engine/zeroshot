/**
 * IsolationManager - Docker container lifecycle for isolated cluster execution
 *
 * Handles:
 * - Container creation with workspace mounts
 * - Credential injection for provider CLIs
 * - Command execution inside containers
 * - Container cleanup on stop/kill
 */

const { spawn } = require('child_process');
const { execSync } = require('./lib/safe-exec'); // Enforces timeouts - prevents infinite hangs
const { Worker } = require('worker_threads');
const crypto = require('crypto');
const path = require('path');
const os = require('os');
const fs = require('fs');
const { loadSettings } = require('../lib/settings');
const { CLAUDE_AUTH_ENV_VARS, resolveClaudeAuth } = require('../lib/settings/claude-auth');
const { normalizeProviderName } = require('../lib/provider-names');
const { resolveMounts, resolveEnvs, expandEnvPatterns } = require('../lib/docker-config');
const { getProvider } = require('./providers');

/**
 * Escape a string for safe use in shell commands
 * Prevents shell injection when passing dynamic values to execSync with shell: true
 * @param {string} str - String to escape
 * @returns {string} Shell-escaped string
 */
function escapeShell(str) {
  // Replace single quotes with escaped version and wrap in single quotes
  // This is the safest approach for shell escaping
  return `'${str.replace(/'/g, "'\\''")}'`;
}

function expandHomePath(value) {
  if (!value) return value;
  if (value === '~') return os.homedir();
  return value.replace(/^~(?=\/|$)/, os.homedir());
}

function pathContains(base, target) {
  const resolvedBase = path.resolve(base);
  const resolvedTarget = path.resolve(target);
  if (resolvedBase === resolvedTarget) return true;
  return resolvedTarget.startsWith(resolvedBase + path.sep);
}

const DEFAULT_IMAGE = 'zeroshot-cluster-base';

class IsolationManager {
  constructor(options = {}) {
    this.image = options.image || DEFAULT_IMAGE;
    this.containers = new Map(); // clusterId -> containerId
    this.isolatedDirs = new Map(); // clusterId -> { path, originalDir }
    this.clusterConfigDirs = new Map(); // clusterId -> configDirPath
    this.worktrees = new Map(); // clusterId -> { path, branch, repoRoot }
  }

  /**
   * Get GitHub token from gh CLI config (hosts.yml)
   * Works with older gh CLI versions that don't have `gh auth token` command
   * @returns {string|null}
   * @private
   */
  _getGhToken() {
    try {
      const hostsPath = path.join(os.homedir(), '.config', 'gh', 'hosts.yml');
      if (!fs.existsSync(hostsPath)) return null;

      const content = fs.readFileSync(hostsPath, 'utf8');
      // Match oauth_token: <token> in YAML
      const match = content.match(/oauth_token:\s*(\S+)/);
      return match ? match[1] : null;
    } catch {
      return null;
    }
  }

  /**
   * Create and start a container for a cluster
   * @param {string} clusterId - Cluster ID
   * @param {object} config - Container config
   * @param {string} config.workDir - Working directory to mount
   * @param {string} [config.image] - Docker image (default: zeroshot-cluster-base)
   * @param {boolean} [config.reuseExistingWorkspace=false] - If true, reuse existing isolated workspace (for resume)
   * @param {Array<string|object>} [config.mounts] - Override default mounts (preset names or {host, container, readonly})
   * @param {boolean} [config.noMounts=false] - Disable all credential mounts
   * @param {string} [config.provider] - Provider name for credential warnings
   * @returns {Promise<string>} Container ID
   */
  async createContainer(clusterId, config) {
    const image = config.image || this.image;
    let workDir = config.workDir || process.cwd();
    const containerName = `zeroshot-cluster-${clusterId}`;
    const reuseExisting = config.reuseExistingWorkspace || false;

    const runningContainerId = this._getRunningContainerId(clusterId);
    if (runningContainerId) {
      return runningContainerId;
    }

    this._removeContainerByName(containerName);

    workDir = await this._prepareIsolatedWorkspace(clusterId, workDir, reuseExisting);

    const settings = loadSettings();
    const providerName = normalizeProviderName(
      config.provider || settings.defaultProvider || 'claude'
    );
    const containerHome = config.containerHome || settings.dockerContainerHome || '/root';

    const clusterConfigDir = this._createClusterConfigDir(clusterId, containerHome);
    console.log(`[IsolationManager] Created cluster config dir at ${clusterConfigDir}`);

    const args = this._buildBaseDockerArgs({
      containerName,
      workDir,
      containerHome,
      clusterConfigDir,
    });

    const mountedHosts = this._applyCredentialMounts(args, config, settings, containerHome);
    this._warnMissingProviderCredentials(providerName, mountedHosts, config, containerHome);

    args.push('-w', '/workspace', image, 'tail', '-f', '/dev/null');

    return this._spawnContainer(clusterId, args, workDir);
  }

  _getRunningContainerId(clusterId) {
    const existingId = this.containers.get(clusterId);
    if (!existingId) {
      return null;
    }

    return this._isContainerRunning(existingId) ? existingId : null;
  }

  async _prepareIsolatedWorkspace(clusterId, workDir, reuseExisting) {
    if (!this._isGitRepo(workDir)) {
      return workDir;
    }

    this.isolatedDirs = this.isolatedDirs || new Map();
    const isolatedPath = path.join(os.tmpdir(), 'zeroshot-isolated', clusterId);

    if (reuseExisting && fs.existsSync(isolatedPath)) {
      console.log(`[IsolationManager] Reusing existing isolated workspace at ${isolatedPath}`);
      this.isolatedDirs.set(clusterId, {
        path: isolatedPath,
        originalDir: workDir,
      });
      return isolatedPath;
    }

    const isolatedDir = await this._createIsolatedCopy(clusterId, workDir);
    this.isolatedDirs.set(clusterId, {
      path: isolatedDir,
      originalDir: workDir,
    });
    console.log(`[IsolationManager] Created isolated copy at ${isolatedDir}`);
    return isolatedDir;
  }

  _buildBaseDockerArgs({ containerName, workDir, containerHome, clusterConfigDir }) {
    return [
      'run',
      '-d',
      '--name',
      containerName,
      '-v',
      `${workDir}:/workspace`,
      '-v',
      '/var/run/docker.sock:/var/run/docker.sock',
      '--group-add',
      this._getDockerGid(),
      '-v',
      `${clusterConfigDir}:${containerHome}/.claude`,
    ];
  }

  _resolveMountConfig(config, settings) {
    if (config.mounts) {
      return config.mounts;
    }

    if (process.env.ZEROSHOT_DOCKER_MOUNTS) {
      try {
        return JSON.parse(process.env.ZEROSHOT_DOCKER_MOUNTS);
      } catch {
        console.warn('[IsolationManager] Invalid ZEROSHOT_DOCKER_MOUNTS JSON, using settings');
        return settings.dockerMounts;
      }
    }

    return settings.dockerMounts;
  }

  _applyCredentialMounts(args, config, settings, containerHome) {
    const mountedHosts = [];
    if (config.noMounts) {
      return mountedHosts;
    }

    const mountConfig = this._resolveMountConfig(config, settings);
    const mounts = resolveMounts(mountConfig, { containerHome });
    const claudeContainerPath = path.posix.join(containerHome, '.claude');

    for (const mount of mounts) {
      if (mount.container === claudeContainerPath) {
        console.warn(
          `[IsolationManager] Skipping mount for ${mount.host} -> ${mount.container} ` +
            '(Claude config is managed by zeroshot).'
        );
        continue;
      }

      const hostPath = expandHomePath(mount.host);

      try {
        const stat = fs.statSync(hostPath);
        if (hostPath.endsWith('config') && !stat.isFile()) {
          continue;
        }
      } catch {
        continue;
      }

      const mountSpec = mount.readonly
        ? `${hostPath}:${mount.container}:ro`
        : `${hostPath}:${mount.container}`;
      args.push('-v', mountSpec);
      mountedHosts.push(hostPath);
    }

    const envToPass = this._collectDockerEnvVars(mountConfig, settings);
    for (const [key, value] of Object.entries(envToPass)) {
      args.push('-e', `${key}=${value}`);
    }

    return mountedHosts;
  }

  _collectDockerEnvVars(mountConfig, settings) {
    const envToPass = {};
    const envSpecs = expandEnvPatterns(resolveEnvs(mountConfig, settings.dockerEnvPassthrough));

    for (const spec of envSpecs) {
      if (spec.forced) {
        envToPass[spec.name] = spec.value;
      } else if (process.env[spec.name]) {
        envToPass[spec.name] = process.env[spec.name];
      }
    }

    for (const envVar of CLAUDE_AUTH_ENV_VARS) {
      if (process.env[envVar]) {
        envToPass[envVar] = process.env[envVar];
      }
    }

    const authEnv = resolveClaudeAuth(settings);
    for (const [key, value] of Object.entries(authEnv)) {
      if (!(key in envToPass)) {
        envToPass[key] = value;
      }
    }

    return envToPass;
  }

  _warnMissingProviderCredentials(providerName, mountedHosts, config, containerHome) {
    if (providerName === 'claude') {
      return;
    }

    const provider = getProvider(providerName);
    const credentialPaths = provider.getCredentialPaths ? provider.getCredentialPaths() : [];
    const expandedCreds = credentialPaths.map((cred) => expandHomePath(cred));
    const hasCredentialMount = mountedHosts.some((hostPath) =>
      expandedCreds.some(
        (credPath) => pathContains(hostPath, credPath) || pathContains(credPath, hostPath)
      )
    );

    if (!hasCredentialMount && expandedCreds.length > 0) {
      const exampleHost = credentialPaths[0];
      const exampleContainer = exampleHost.replace(/^~(?=\/|$)/, containerHome);
      const mountNote = config.noMounts ? 'Credential mounts are disabled. ' : '';
      console.warn(
        `[IsolationManager] ⚠️  ${mountNote}No credential mounts found for ${provider.displayName}. ` +
          `Add one with --mount ${exampleHost}:${exampleContainer}:ro`
      );
    }
  }

  _spawnContainer(clusterId, args, workDir) {
    return new Promise((resolve, reject) => {
      const proc = spawn('docker', args, { stdio: ['pipe', 'pipe', 'pipe'] });

      let stdout = '';
      let stderr = '';

      proc.stdout.on('data', (data) => {
        stdout += data;
      });
      proc.stderr.on('data', (data) => {
        stderr += data;
      });

      proc.on('close', async (code) => {
        if (code !== 0) {
          reject(new Error(`Failed to create container: ${stderr}`));
          return;
        }

        const containerId = stdout.trim().substring(0, 12);
        this.containers.set(clusterId, containerId);

        try {
          console.log(`[IsolationManager] Checking for package.json in ${workDir}...`);
          if (fs.existsSync(path.join(workDir, 'package.json'))) {
            await this._installDependenciesWithRetry(clusterId);
          }
        } catch (err) {
          console.warn(
            `[IsolationManager] ⚠️ Failed to install dependencies (non-fatal): ${err.message}`
          );
        }

        resolve(containerId);
      });

      proc.on('error', (err) => {
        reject(new Error(`Docker spawn error: ${err.message}`));
      });
    });
  }

  async _installDependenciesWithRetry(clusterId) {
    console.log(`[IsolationManager] Installing npm dependencies in container...`);

    const maxRetries = 3;
    const baseDelay = 2000; // 2 seconds
    const installCommand = [
      'sh',
      '-c',
      [
        'if [ -d node_modules ] && [ -f node_modules/.package-lock.json ]; then',
        'echo "__deps_present__";',
        'exit 0;',
        'fi;',
        'if ! command -v npm >/dev/null 2>&1; then',
        'echo "__npm_missing__";',
        'exit 127;',
        'fi;',
        'if [ -d /pre-baked-deps/node_modules ]; then',
        'cp -rn /pre-baked-deps/node_modules . 2>/dev/null || true;',
        'npm_config_engine_strict=false npm install --no-audit --no-fund --prefer-offline;',
        'install_code=$?;',
        'if [ $install_code -ne 0 ]; then',
        'rm -rf node_modules;',
        'npm_config_engine_strict=false npm install --no-audit --no-fund;',
        'fi;',
        'else',
        'npm_config_engine_strict=false npm install --no-audit --no-fund;',
        'fi',
      ].join(' '),
    ];

    for (let attempt = 1; attempt <= maxRetries; attempt++) {
      try {
        const installResult = await this.execInContainer(clusterId, installCommand, {});
        const stdout = installResult.stdout || '';

        if (installResult.code === 0) {
          if (stdout.includes('__deps_present__')) {
            console.log(
              `[IsolationManager] ✓ Dependencies already installed (skipping npm install)`
            );
          } else {
            console.log(`[IsolationManager] ✓ Dependencies installed`);
          }
          return;
        }

        const errorOutput = (installResult.stderr || installResult.stdout || '').slice(0, 500);
        if (attempt < maxRetries) {
          const delay = baseDelay * Math.pow(2, attempt - 1);
          console.warn(
            `[IsolationManager] ⚠️ npm install failed (attempt ${attempt}/${maxRetries}), retrying in ${delay}ms...`
          );
          console.warn(`[IsolationManager] Error: ${errorOutput}`);
          await new Promise((resolve) => setTimeout(resolve, delay));
        } else {
          console.warn(
            `[IsolationManager] ⚠️ npm install failed after ${maxRetries} attempts (non-fatal): ${errorOutput}`
          );
        }
      } catch (execErr) {
        if (attempt < maxRetries) {
          const delay = baseDelay * Math.pow(2, attempt - 1);
          console.warn(
            `[IsolationManager] ⚠️ npm install execution error (attempt ${attempt}/${maxRetries}), retrying in ${delay}ms...`
          );
          console.warn(`[IsolationManager] Error: ${execErr.message}`);
          await new Promise((resolve) => setTimeout(resolve, delay));
        } else {
          throw execErr;
        }
      }
    }
  }

  /**
   * Execute a command inside the container
   * @param {string} clusterId - Cluster ID
   * @param {string[]} command - Command and arguments
   * @param {object} [options] - Exec options
   * @param {boolean} [options.interactive] - Use -it flags
   * @param {object} [options.env] - Environment variables
   * @param {number} [options.timeout=30000] - Timeout in ms (0 = no timeout). Prevents infinite hangs.
   * @returns {Promise<{stdout: string, stderr: string, code: number}>}
   */
  execInContainer(clusterId, command, options = {}) {
    const containerId = this.containers.get(clusterId);
    if (!containerId) {
      throw new Error(`No container found for cluster ${clusterId}`);
    }

    const args = ['exec'];

    if (options.interactive) {
      args.push('-it');
    }

    // Add environment variables
    if (options.env) {
      for (const [key, value] of Object.entries(options.env)) {
        args.push('-e', `${key}=${value}`);
      }
    }

    args.push(containerId, ...command);

    // Default timeout: 30 seconds (prevents infinite hangs)
    const timeout = options.timeout ?? 30000;

    return new Promise((resolve, reject) => {
      const proc = spawn('docker', args, {
        stdio: options.interactive ? 'inherit' : ['pipe', 'pipe', 'pipe'],
      });

      let stdout = '';
      let stderr = '';
      let timedOut = false;
      let timeoutId = null;

      // Set up timeout if specified (0 = no timeout)
      if (timeout > 0) {
        timeoutId = setTimeout(() => {
          timedOut = true;
          proc.kill('SIGKILL');
        }, timeout);
      }

      if (!options.interactive) {
        proc.stdout.on('data', (data) => {
          stdout += data;
        });
        proc.stderr.on('data', (data) => {
          stderr += data;
        });
      }

      proc.on('close', (code) => {
        if (timeoutId) clearTimeout(timeoutId);
        if (timedOut) {
          reject(new Error(`Docker exec timed out after ${timeout}ms`));
        } else {
          resolve({ stdout, stderr, code });
        }
      });

      proc.on('error', (err) => {
        if (timeoutId) clearTimeout(timeoutId);
        reject(new Error(`Docker exec error: ${err.message}`));
      });
    });
  }

  /**
   * Spawn a PTY-like process inside the container
   * Returns a child process that can be used like a PTY
   * @param {string} clusterId - Cluster ID
   * @param {string[]} command - Command and arguments
   * @param {object} [options] - Spawn options
   * @returns {ChildProcess}
   */
  spawnInContainer(clusterId, command, options = {}) {
    const containerId = this.containers.get(clusterId);
    if (!containerId) {
      throw new Error(`No container found for cluster ${clusterId}`);
    }

    // IMPORTANT: Must use -i flag for interactive stdin/stdout communication with commands like 'cat'
    // If omitted, docker exec will not properly connect stdin, causing piped input to be ignored
    // This is required for PTY-like behavior where child process stdin/stdout are used
    const args = ['exec', '-i'];

    // Add environment variables
    if (options.env) {
      for (const [key, value] of Object.entries(options.env)) {
        args.push('-e', `${key}=${value}`);
      }
    }

    args.push(containerId, ...command);

    return spawn('docker', args, {
      stdio: ['pipe', 'pipe', 'pipe'],
      ...options.spawnOptions,
    });
  }

  /**
   * Stop a container
   * @param {string} clusterId - Cluster ID
   * @param {number} [timeout=10] - Timeout in seconds before SIGKILL
   * @returns {Promise<void>}
   */
  stopContainer(clusterId, timeout = 10, explicitContainerId = null) {
    // Use explicit containerId (from restored state) or in-memory Map
    const containerId = explicitContainerId || this.containers.get(clusterId);
    if (!containerId) {
      return; // Already stopped or never started
    }

    return new Promise((resolve) => {
      const proc = spawn('docker', ['stop', '-t', String(timeout), containerId], {
        stdio: ['pipe', 'pipe', 'pipe'],
      });

      proc.on('close', () => {
        resolve();
      });

      proc.on('error', () => {
        resolve(); // Ignore errors on stop
      });
    });
  }

  /**
   * Remove a container
   * @param {string} clusterId - Cluster ID
   * @param {boolean} [force=false] - Force remove running container
   * @returns {Promise<void>}
   */
  removeContainer(clusterId, force = false, explicitContainerId = null) {
    // Use explicit containerId (from restored state) or in-memory Map
    const containerId = explicitContainerId || this.containers.get(clusterId);
    if (!containerId) {
      return;
    }

    const args = ['rm'];
    if (force) {
      args.push('-f');
    }
    args.push(containerId);

    return new Promise((resolve) => {
      const proc = spawn('docker', args, {
        stdio: ['pipe', 'pipe', 'pipe'],
      });

      proc.on('close', () => {
        this.containers.delete(clusterId);
        resolve();
      });

      proc.on('error', () => {
        this.containers.delete(clusterId);
        resolve();
      });
    });
  }

  /**
   * Stop and remove a container, and optionally clean up isolated dir/config
   * @param {string} clusterId - Cluster ID
   * @param {object} [options] - Cleanup options
   * @param {boolean} [options.preserveWorkspace=false] - If true, keep the isolated workspace (for resume capability)
   * @returns {Promise<void>}
   */
  async cleanup(clusterId, options = {}) {
    const preserveWorkspace = options.preserveWorkspace || false;

    await this.stopContainer(clusterId);
    await this.removeContainer(clusterId);

    // Clean up isolated directory if one was created (unless preserveWorkspace is set)
    if (this.isolatedDirs?.has(clusterId)) {
      const isolatedInfo = this.isolatedDirs.get(clusterId);

      if (preserveWorkspace) {
        console.log(
          `[IsolationManager] Preserving isolated workspace at ${isolatedInfo.path} for resume`
        );
        // Don't delete - but DON'T remove from Map either, resume() needs it
      } else {
        console.log(`[IsolationManager] Cleaning up isolated dir at ${isolatedInfo.path}`);

        // Preserve Terraform state before deleting isolated directory
        this._preserveTerraformState(clusterId, isolatedInfo.path);

        // Remove the isolated directory
        try {
          fs.rmSync(isolatedInfo.path, { recursive: true, force: true });
        } catch {
          // Ignore
        }
        this.isolatedDirs.delete(clusterId);
      }
    }

    // Clean up cluster config dir (always - it's recreated on resume)
    this._cleanupClusterConfigDir(clusterId);
  }

  /**
   * Create an isolated copy of a directory with fresh git repo
   * @private
   * @param {string} clusterId - Cluster ID
   * @param {string} sourceDir - Source directory to copy
   * @returns {Promise<string>} Path to isolated directory
   */
  async _createIsolatedCopy(clusterId, sourceDir) {
    const isolatedPath = path.join(os.tmpdir(), 'zeroshot-isolated', clusterId);

    // Clean up existing dir
    if (fs.existsSync(isolatedPath)) {
      fs.rmSync(isolatedPath, { recursive: true, force: true });
    }

    // Create directory
    fs.mkdirSync(isolatedPath, { recursive: true });

    // Copy files (excluding .git and common build artifacts)
    await this._copyDirExcluding(sourceDir, isolatedPath, [
      '.git',
      'node_modules',
      '.next',
      'dist',
      'build',
      '__pycache__',
      '.pytest_cache',
      '.mypy_cache',
      '.ruff_cache',
      '.venv',
      'venv',
      '.tox',
      '.eggs',
      '*.egg-info',
      'coverage',
      '.coverage',
      '.nyc_output',
      '.DS_Store',
      'Thumbs.db',
    ]);

    // Get remote URL from original repo (for PR creation)
    let remoteUrl = null;
    try {
      remoteUrl = execSync('git remote get-url origin', {
        cwd: sourceDir,
        encoding: 'utf8',
        stdio: 'pipe',
      }).trim();
    } catch {
      // No remote configured in source
    }

    // Initialize fresh git repo with all setup in a single batched command
    // This reduces ~500ms overhead (5 execSync calls @ ~100ms each) to ~100ms (1 call)
    // Issue #22: Batch git operations for 5-10% startup reduction
    const branchName = `zeroshot/${clusterId}`;

    // Build authenticated remote URL if source had one (needed for git push / PR creation)
    let authRemoteUrl = null;
    if (remoteUrl) {
      authRemoteUrl = remoteUrl;
      const token = this._getGhToken();
      if (token && remoteUrl.startsWith('https://github.com/')) {
        // Convert https://github.com/org/repo.git to https://x-access-token:TOKEN@github.com/org/repo.git
        authRemoteUrl = remoteUrl.replace(
          'https://github.com/',
          `https://x-access-token:${token}@github.com/`
        );
      }
    }

    // Batch all git operations into a single shell command
    // Using --allow-empty on commit to handle edge case of empty directories
    const gitCommands = [
      'git init',
      authRemoteUrl ? `git remote add origin ${escapeShell(authRemoteUrl)}` : null,
      'git add -A',
      'git commit -m "Initial commit (isolated copy)" --allow-empty',
      `git checkout -b ${escapeShell(branchName)}`,
    ]
      .filter(Boolean)
      .join(' && ');

    execSync(gitCommands, {
      cwd: isolatedPath,
      stdio: 'pipe',
      shell: '/bin/bash',
    });

    return isolatedPath;
  }

  /**
   * Copy directory excluding certain paths using parallel worker threads
   * Supports exact matches and glob patterns (*.ext)
   *
   * Performance optimization for large repos (10k+ files):
   * - Phase 1: Collect all files async (non-blocking traversal)
   * - Phase 2: Create directory structure (must be sequential)
   * - Phase 3: Copy files in parallel using worker threads
   *
   * @private
   * @param {string} src - Source directory
   * @param {string} dest - Destination directory
   * @param {string[]} exclude - Patterns to exclude
   * @returns {Promise<void>}
   */
  async _copyDirExcluding(src, dest, exclude) {
    // Phase 1: Collect all files and directories
    const files = [];
    const directories = new Set();

    const shouldIgnoreFsError = (err) =>
      err.code === 'EACCES' || err.code === 'EPERM' || err.code === 'ENOENT';

    const shouldExcludeEntry = (entryName) => {
      return exclude.some((pattern) => {
        if (pattern.startsWith('*.')) {
          return entryName.endsWith(pattern.slice(1));
        }
        return entryName === pattern;
      });
    };

    const ensureParentDirTracked = (relativePath) => {
      if (relativePath) {
        directories.add(relativePath);
      }
    };

    const readEntries = (currentSrc) => {
      try {
        return fs.readdirSync(currentSrc, { withFileTypes: true });
      } catch (err) {
        if (shouldIgnoreFsError(err)) {
          return [];
        }
        throw err;
      }
    };

    function handleEntry(entry, srcPath, relPath, relativePath) {
      if (entry.isSymbolicLink()) {
        const targetStats = fs.statSync(srcPath);
        if (targetStats.isDirectory()) {
          directories.add(relPath);
          collectFiles(srcPath, relPath);
          return;
        }

        files.push(relPath);
        ensureParentDirTracked(relativePath);
        return;
      }

      if (entry.isDirectory()) {
        directories.add(relPath);
        collectFiles(srcPath, relPath);
        return;
      }

      files.push(relPath);
      ensureParentDirTracked(relativePath);
    }

    function collectFiles(currentSrc, relativePath = '') {
      const entries = readEntries(currentSrc);

      for (const entry of entries) {
        if (shouldExcludeEntry(entry.name)) {
          continue;
        }

        const srcPath = path.join(currentSrc, entry.name);
        const relPath = relativePath ? path.join(relativePath, entry.name) : entry.name;

        try {
          handleEntry(entry, srcPath, relPath, relativePath);
        } catch (err) {
          if (shouldIgnoreFsError(err)) {
            continue;
          }
          throw err;
        }
      }
    }

    collectFiles(src);

    // Phase 2: Create directory structure (sequential - must exist before file copy)
    // Sort directories by depth to ensure parents are created before children
    const sortedDirs = Array.from(directories).sort((a, b) => {
      const depthA = a.split(path.sep).length;
      const depthB = b.split(path.sep).length;
      return depthA - depthB;
    });

    for (const dir of sortedDirs) {
      const destDir = path.join(dest, dir);
      try {
        fs.mkdirSync(destDir, { recursive: true });
      } catch (err) {
        if (err.code !== 'EEXIST') {
          throw err;
        }
      }
    }

    // Phase 3: Copy files in parallel using worker threads
    // For small file counts (<100), use synchronous copy (worker overhead not worth it)
    if (files.length < 100) {
      for (const relPath of files) {
        const srcPath = path.join(src, relPath);
        const destPath = path.join(dest, relPath);
        try {
          fs.copyFileSync(srcPath, destPath);
        } catch (err) {
          if (err.code !== 'EACCES' && err.code !== 'EPERM' && err.code !== 'ENOENT') {
            throw err;
          }
        }
      }
      return;
    }

    // Use worker threads for larger file counts
    const numWorkers = Math.min(4, os.cpus().length);
    const chunkSize = Math.ceil(files.length / numWorkers);
    const workerPath = path.join(__dirname, 'copy-worker.js');

    // Split files into chunks for workers
    const chunks = [];
    for (let i = 0; i < files.length; i += chunkSize) {
      chunks.push(files.slice(i, i + chunkSize));
    }

    // Spawn workers and wait for completion
    const workerPromises = chunks.map((chunk) => {
      return new Promise((resolve, reject) => {
        const worker = new Worker(workerPath, {
          workerData: {
            files: chunk,
            sourceBase: src,
            destBase: dest,
          },
        });

        worker.on('message', (result) => {
          resolve(result);
        });

        worker.on('error', (err) => {
          reject(err);
        });

        worker.on('exit', (code) => {
          if (code !== 0) {
            reject(new Error(`Worker exited with code ${code}`));
          }
        });
      });
    });

    // Wait for all workers to complete (proper async/await - no busy-wait!)
    // FIX: Previous version used busy-wait which blocked the event loop,
    // preventing worker thread messages from being processed (timeout bug)
    await Promise.all(workerPromises);
  }

  /**
   * Get container ID for a cluster
   * @param {string} clusterId - Cluster ID
   * @returns {string|undefined}
   */
  getContainerId(clusterId) {
    return this.containers.get(clusterId);
  }

  /**
   * Check if a cluster has an active container
   * @param {string} clusterId - Cluster ID
   * @returns {boolean}
   */
  hasContainer(clusterId) {
    const containerId = this.containers.get(clusterId);
    if (!containerId) return false;
    return this._isContainerRunning(containerId);
  }

  /**
   * Get Claude config directory
   * @private
   */
  _getClaudeConfigDir() {
    return process.env.CLAUDE_CONFIG_DIR || path.join(os.homedir(), '.claude');
  }

  /**
   * Create a fresh Claude config directory for a cluster (avoids permission issues from host)
   * Copies only essential files: .credentials.json
   * @private
   * @param {string} clusterId - Cluster ID
   * @param {string} containerHome - Container home directory path (e.g., '/root' or '/home/node')
   * @returns {string} Path to cluster-specific config directory
   */
  _createClusterConfigDir(clusterId, containerHome = '/root') {
    const sourceDir = this._getClaudeConfigDir();
    const configDir = path.join(os.tmpdir(), 'zeroshot-cluster-configs', clusterId);

    // Clean up existing dir
    if (fs.existsSync(configDir)) {
      fs.rmSync(configDir, { recursive: true, force: true });
    }

    // Create fresh directory and required subdirectories
    fs.mkdirSync(configDir, { recursive: true });
    const hooksDir = path.join(configDir, 'hooks');
    fs.mkdirSync(hooksDir, { recursive: true });
    // CRITICAL: Claude CLI writes session files to projects/ subdirectory
    const projectsDir = path.join(configDir, 'projects');
    fs.mkdirSync(projectsDir, { recursive: true });

    // Copy only credentials file (essential for auth)
    const credentialsFile = path.join(sourceDir, '.credentials.json');
    if (fs.existsSync(credentialsFile)) {
      fs.copyFileSync(credentialsFile, path.join(configDir, '.credentials.json'));
    }

    // Copy hook script to block AskUserQuestion (CRITICAL for autonomous execution)
    const hookScriptSrc = path.join(__dirname, '..', 'hooks', 'block-ask-user-question.py');
    const hookScriptDst = path.join(hooksDir, 'block-ask-user-question.py');
    if (fs.existsSync(hookScriptSrc)) {
      fs.copyFileSync(hookScriptSrc, hookScriptDst);
      fs.chmodSync(hookScriptDst, 0o755);
    }

    // Create settings.json with PreToolUse hook to block AskUserQuestion
    // This PREVENTS agents from asking questions in non-interactive mode
    const clusterSettings = {
      hooks: {
        PreToolUse: [
          {
            matcher: 'AskUserQuestion',
            hooks: [
              {
                type: 'command',
                command: `${containerHome}/.claude/hooks/block-ask-user-question.py`,
              },
            ],
          },
        ],
      },
    };
    fs.writeFileSync(
      path.join(configDir, 'settings.json'),
      JSON.stringify(clusterSettings, null, 2)
    );

    // Track for cleanup
    this.clusterConfigDirs = this.clusterConfigDirs || new Map();
    this.clusterConfigDirs.set(clusterId, configDir);

    return configDir;
  }

  /**
   * Clean up cluster config directory
   * @private
   * @param {string} clusterId - Cluster ID
   */
  _cleanupClusterConfigDir(clusterId) {
    if (!this.clusterConfigDirs?.has(clusterId)) return;

    const configDir = this.clusterConfigDirs.get(clusterId);
    try {
      fs.rmSync(configDir, { recursive: true, force: true });
    } catch {
      // Ignore
    }
    this.clusterConfigDirs.delete(clusterId);
  }

  /**
   * Preserve Terraform state files before cleanup
   * Checks both terraform/ subdirectory and root directory
   * @private
   * @param {string} clusterId - Cluster ID
   * @param {string} isolatedPath - Path to isolated directory
   */
  _preserveTerraformState(clusterId, isolatedPath) {
    const stateFiles = ['terraform.tfstate', 'terraform.tfstate.backup', 'tfplan'];
    const checkDirs = [isolatedPath, path.join(isolatedPath, 'terraform')];
    const stateDir = path.join(os.homedir(), '.zeroshot', 'terraform-state', clusterId);

    const hasStateFiles = (checkDir) => {
      if (!fs.existsSync(checkDir)) {
        return false;
      }

      return stateFiles.some((file) => fs.existsSync(path.join(checkDir, file)));
    };

    const copyStateFiles = (checkDir) => {
      let copied = false;

      for (const file of stateFiles) {
        const srcPath = path.join(checkDir, file);
        if (!fs.existsSync(srcPath)) {
          continue;
        }

        const destPath = path.join(stateDir, file);
        try {
          fs.copyFileSync(srcPath, destPath);
          console.log(`[IsolationManager] Preserved Terraform state: ${file} → ${stateDir}`);
          copied = true;
        } catch (err) {
          console.warn(`[IsolationManager] Failed to preserve ${file}: ${err.message}`);
        }
      }

      return copied;
    };

    let foundState = false;

    for (const checkDir of checkDirs) {
      if (!hasStateFiles(checkDir)) {
        continue;
      }

      fs.mkdirSync(stateDir, { recursive: true });
      foundState = copyStateFiles(checkDir);
      break;
    }

    if (!foundState) {
      console.log(`[IsolationManager] No Terraform state found to preserve`);
    }
  }

  /**
   * Get host's docker group GID (for Docker socket access inside container)
   * @private
   * @returns {string} Docker group GID
   */
  _getDockerGid() {
    try {
      // Get docker group info: "docker:x:999:user1,user2"
      const result = execSync('getent group docker', { encoding: 'utf8' });
      const gid = result.split(':')[2];
      return gid.trim();
    } catch {
      // Fallback: common docker GID is 999
      console.warn('[IsolationManager] Could not detect docker GID, using default 999');
      return '999';
    }
  }

  /**
   * Check if a container is running
   * @private
   */
  _isContainerRunning(containerId) {
    try {
      const result = execSync(
        `docker inspect -f '{{.State.Running}}' ${escapeShell(containerId)} 2>/dev/null`,
        {
          encoding: 'utf8',
        }
      );
      return result.trim() === 'true';
    } catch {
      return false;
    }
  }

  /**
   * Remove container by name (cleanup before create)
   * @private
   */
  _removeContainerByName(name) {
    try {
      execSync(`docker rm -f ${escapeShell(name)} 2>/dev/null`, { encoding: 'utf8' });
    } catch {
      // Ignore - container doesn't exist
    }
  }

  /**
   * Check if Docker is available
   * @returns {boolean}
   */
  static isDockerAvailable() {
    try {
      // Require both CLI binary and a reachable daemon.
      execSync('docker info', { encoding: 'utf8', stdio: 'pipe' });
      return true;
    } catch {
      return false;
    }
  }

  /**
   * Check if the base image exists
   * @param {string} [image] - Image name to check
   * @returns {boolean}
   */
  static imageExists(image = DEFAULT_IMAGE) {
    try {
      execSync(`docker image inspect ${escapeShell(image)} 2>/dev/null`, {
        encoding: 'utf8',
        stdio: 'pipe',
      });
      return true;
    } catch {
      return false;
    }
  }

  /**
   * Build the Docker image with retry logic
   * @param {string} [image] - Image name to build
   * @param {number} [maxRetries=3] - Maximum retry attempts
   * @returns {Promise<void>}
   */
  static async buildImage(image = DEFAULT_IMAGE, maxRetries = 3) {
    // Repository root is one level up from src/
    const repoRoot = path.join(__dirname, '..');
    const dockerfilePath = path.join(repoRoot, 'docker', 'zeroshot-cluster', 'Dockerfile');

    if (!fs.existsSync(dockerfilePath)) {
      throw new Error(`Dockerfile not found at ${dockerfilePath}`);
    }

    console.log(`[IsolationManager] Building Docker image '${image}'...`);

    const baseDelay = 3000; // 3 seconds

    for (let attempt = 1; attempt <= maxRetries; attempt++) {
      try {
        // CRITICAL: Run from repo root so build context includes package.json and src/
        // Use -f flag to specify Dockerfile location
        execSync(`docker build -f docker/zeroshot-cluster/Dockerfile -t ${escapeShell(image)} .`, {
          cwd: repoRoot,
          encoding: 'utf8',
          stdio: 'inherit',
        });

        console.log(`[IsolationManager] ✓ Image '${image}' built successfully`);
        return;
      } catch (err) {
        if (attempt < maxRetries) {
          const delay = baseDelay * Math.pow(2, attempt - 1);
          console.warn(
            `[IsolationManager] ⚠️ Docker build failed (attempt ${attempt}/${maxRetries}), retrying in ${delay}ms...`
          );
          console.warn(`[IsolationManager] Error: ${err.message}`);
          await new Promise((resolve) => setTimeout(resolve, delay));
        } else {
          throw new Error(
            `Failed to build Docker image '${image}' after ${maxRetries} attempts: ${err.message}`
          );
        }
      }
    }
  }

  /**
   * Ensure Docker image exists, building it if necessary
   * @param {string} [image] - Image name to ensure
   * @param {boolean} [autoBuild=true] - Auto-build if missing
   * @returns {Promise<void>}
   */
  static async ensureImage(image = DEFAULT_IMAGE, autoBuild = true) {
    if (this.imageExists(image)) {
      return;
    }

    if (!autoBuild) {
      throw new Error(
        `Docker image '${image}' not found. Build it with:\n` +
          `  docker build -t ${image} zeroshot/cluster/docker/zeroshot-cluster/`
      );
    }

    console.log(`[IsolationManager] Image '${image}' not found, building automatically...`);
    await this.buildImage(image);
  }

  /**
   * Check if directory is a git repository
   * @private
   */
  _isGitRepo(dir) {
    try {
      execSync('git rev-parse --git-dir', {
        cwd: dir,
        encoding: 'utf8',
        stdio: 'pipe',
      });
      return true;
    } catch {
      return false;
    }
  }

  /**
   * Get the git repository root for a directory
   * @private
   */
  _getGitRoot(dir) {
    try {
      return execSync('git rev-parse --show-toplevel', {
        cwd: dir,
        encoding: 'utf8',
        stdio: 'pipe',
      }).trim();
    } catch {
      return null;
    }
  }

  /**
   * Create worktree-based isolation for a cluster (lightweight alternative to Docker)
   * Creates a git worktree at {os.tmpdir()}/zeroshot-worktrees/{clusterId}
   * @param {string} clusterId - Cluster ID
   * @param {string} workDir - Original working directory (must be a git repo)
   * @returns {{ path: string, branch: string, repoRoot: string }}
   */
  createWorktreeIsolation(clusterId, workDir) {
    if (!this._isGitRepo(workDir)) {
      throw new Error(
        `Worktree isolation requires a git repository. ${workDir} is not a git repo.`
      );
    }

    const worktreeInfo = this.createWorktree(clusterId, workDir);
    this.worktrees.set(clusterId, worktreeInfo);

    console.log(`[IsolationManager] Created worktree isolation at ${worktreeInfo.path}`);
    console.log(`[IsolationManager] Branch: ${worktreeInfo.branch}`);

    return worktreeInfo;
  }

  /**
   * Clean up worktree isolation for a cluster
   * @param {string} clusterId - Cluster ID
   * @param {object} [options] - Cleanup options
   * @param {boolean} [options.preserveBranch=true] - Keep the branch after removing worktree
   */
  cleanupWorktreeIsolation(clusterId, options = {}) {
    const worktreeInfo = this.worktrees.get(clusterId);
    if (!worktreeInfo) {
      return; // No worktree to clean up
    }

    this.removeWorktree(worktreeInfo, options);
    this.worktrees.delete(clusterId);

    console.log(`[IsolationManager] Cleaned up worktree isolation for ${clusterId}`);
  }

  /**
   * Create a git worktree for isolated work
   * @param {string} clusterId - Cluster ID (used as branch name)
   * @param {string} workDir - Original working directory
   * @returns {{ path: string, branch: string, repoRoot: string }}
   */
  createWorktree(clusterId, workDir) {
    const repoRoot = this._getGitRoot(workDir);
    if (!repoRoot) {
      throw new Error(`Cannot find git root for ${workDir}`);
    }

    // Create branch name from cluster ID (e.g., cluster-cosmic-meteor-87 -> zeroshot/cosmic-meteor-87)
    const baseBranchName = `zeroshot/${clusterId.replace(/^cluster-/, '')}`;
    let branchName = baseBranchName;

    // Worktree path in tmp
    const worktreePath = path.join(os.tmpdir(), 'zeroshot-worktrees', clusterId);

    // Ensure parent directory exists
    const parentDir = path.dirname(worktreePath);
    if (!fs.existsSync(parentDir)) {
      fs.mkdirSync(parentDir, { recursive: true });
    }

    // Best-effort cleanup of stale worktree metadata and directory.
    // IMPORTANT: If a previous run deleted the directory without deregistering the worktree,
    // git may keep the branch "checked out" and block deletion/reuse.
    try {
      execSync(`git worktree remove --force ${escapeShell(worktreePath)}`, {
        cwd: repoRoot,
        encoding: 'utf8',
        stdio: 'pipe',
      });
    } catch {
      // ignore
    }
    try {
      execSync('git worktree prune', { cwd: repoRoot, encoding: 'utf8', stdio: 'pipe' });
    } catch {
      // ignore
    }
    try {
      fs.rmSync(worktreePath, { recursive: true, force: true });
    } catch {
      // ignore
    }

    // Create worktree with new branch based on HEAD (retry on branch collision/in-use)
    for (let attempt = 0; attempt < 10; attempt++) {
      // Best-effort delete if branch exists and is not in use by another worktree.
      try {
        execSync(`git branch -D ${escapeShell(branchName)}`, {
          cwd: repoRoot,
          encoding: 'utf8',
          stdio: 'pipe',
        });
      } catch {
        // ignore
      }

      try {
        execSync(
          `git worktree add -b ${escapeShell(branchName)} ${escapeShell(worktreePath)} HEAD`,
          {
            cwd: repoRoot,
            encoding: 'utf8',
            stdio: 'pipe',
          }
        );
        break;
      } catch (err) {
        const stderr = (
          err && (err.stderr || err.message) ? String(err.stderr || err.message) : ''
        ).toLowerCase();
        const isBranchCollision =
          stderr.includes('already exists') ||
          stderr.includes('cannot delete branch') ||
          stderr.includes('checked out');

        if (attempt < 9 && isBranchCollision) {
          branchName = `${baseBranchName}-${crypto.randomBytes(3).toString('hex')}`;
          try {
            execSync('git worktree prune', { cwd: repoRoot, encoding: 'utf8', stdio: 'pipe' });
          } catch {
            // ignore
          }
          continue;
        }
        throw err;
      }
    }

    return {
      path: worktreePath,
      branch: branchName,
      repoRoot,
    };
  }

  /**
   * Remove a git worktree
   * @param {{ path: string, branch: string, repoRoot: string }} worktreeInfo
   * @param {object} [options] - Removal options
   * @param {boolean} [options.deleteBranch=false] - Also delete the branch
   */
  removeWorktree(worktreeInfo, _options = {}) {
    // Remove the worktree (prefer git so metadata is cleaned up).
    try {
      execSync(`git worktree remove --force ${escapeShell(worktreeInfo.path)}`, {
        cwd: worktreeInfo.repoRoot,
        encoding: 'utf8',
        stdio: 'pipe',
      });
    } catch {
      // If git worktree metadata is stale, prune and retry once.
      try {
        execSync('git worktree prune', {
          cwd: worktreeInfo.repoRoot,
          encoding: 'utf8',
          stdio: 'pipe',
        });
      } catch {
        // ignore
      }
      try {
        execSync(`git worktree remove --force ${escapeShell(worktreeInfo.path)}`, {
          cwd: worktreeInfo.repoRoot,
          encoding: 'utf8',
          stdio: 'pipe',
        });
      } catch {
        // Last resort: delete directory, then prune stale worktree entries.
        try {
          fs.rmSync(worktreeInfo.path, { recursive: true, force: true });
        } catch {
          // ignore
        }
        try {
          execSync('git worktree prune', {
            cwd: worktreeInfo.repoRoot,
            encoding: 'utf8',
            stdio: 'pipe',
          });
        } catch {
          // ignore
        }
      }
    }

    // Optionally delete the branch (only if not merged)
    // We leave this commented out - let the user decide to keep/delete branches
    // try {
    //   execSync(`git branch -D "${worktreeInfo.branch}" 2>/dev/null`, {
    //     cwd: worktreeInfo.repoRoot,
    //     encoding: 'utf8',
    //     stdio: 'pipe'
    //   });
    // } catch {
    //   // Ignore - branch may have been merged or deleted
    // }
  }

  /**
   * Get worktree info for a cluster
   * @param {string} clusterId - Cluster ID
   * @returns {{ path: string, branch: string, repoRoot: string }|undefined}
   */
  getWorktreeInfo(clusterId) {
    return this.worktrees.get(clusterId);
  }
}

module.exports = IsolationManager;
