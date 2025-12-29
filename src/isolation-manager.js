/**
 * IsolationManager - Docker container lifecycle for isolated cluster execution
 *
 * Handles:
 * - Container creation with workspace mounts
 * - Credential injection for Claude CLI
 * - Command execution inside containers
 * - Container cleanup on stop/kill
 */

const { spawn, execSync } = require('child_process');
const { Worker } = require('worker_threads');
const path = require('path');
const os = require('os');
const fs = require('fs');

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

const DEFAULT_IMAGE = 'zeroshot-cluster-base';

class IsolationManager {
  constructor(options = {}) {
    this.image = options.image || DEFAULT_IMAGE;
    this.containers = new Map(); // clusterId -> containerId
    this.isolatedDirs = new Map(); // clusterId -> { path, originalDir }
    this.clusterConfigDirs = new Map(); // clusterId -> configDirPath
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
   * @returns {Promise<string>} Container ID
   */
  createContainer(clusterId, config) {
    const image = config.image || this.image;
    let workDir = config.workDir || process.cwd();
    const containerName = `zeroshot-cluster-${clusterId}`;
    const reuseExisting = config.reuseExistingWorkspace || false;

    // Check if container already exists
    if (this.containers.has(clusterId)) {
      const existingId = this.containers.get(clusterId);
      if (this._isContainerRunning(existingId)) {
        return existingId;
      }
    }

    // Clean up any existing container with same name
    this._removeContainerByName(containerName);

    // For isolation mode: copy files to temp dir with fresh git repo (100% isolated)
    // No worktrees - cleaner, no host path dependencies
    // EXCEPTION: On resume (reuseExisting=true), skip copy and use existing workspace
    if (this._isGitRepo(workDir)) {
      const isolatedPath = path.join(os.tmpdir(), 'zeroshot-isolated', clusterId);

      if (reuseExisting && fs.existsSync(isolatedPath)) {
        // Resume mode: reuse existing isolated workspace (contains agent's work)
        console.log(`[IsolationManager] Reusing existing isolated workspace at ${isolatedPath}`);
        this.isolatedDirs = this.isolatedDirs || new Map();
        this.isolatedDirs.set(clusterId, {
          path: isolatedPath,
          originalDir: workDir,
        });
        workDir = isolatedPath;
      } else {
        // Fresh start: create new isolated copy
        const isolatedDir = this._createIsolatedCopy(clusterId, workDir);
        this.isolatedDirs = this.isolatedDirs || new Map();
        this.isolatedDirs.set(clusterId, {
          path: isolatedDir,
          originalDir: workDir,
        });
        workDir = isolatedDir;
        console.log(`[IsolationManager] Created isolated copy at ${workDir}`);
      }
    }

    // Create fresh Claude config dir for this cluster (avoids permission issues from host)
    const clusterConfigDir = this._createClusterConfigDir(clusterId);
    console.log(`[IsolationManager] Created cluster config dir at ${clusterConfigDir}`);

    // Build docker run command
    // NOTE: Container runs as 'node' user (uid 1000) for --dangerously-skip-permissions
    const args = [
      'run',
      '-d', // detached
      '--name',
      containerName,
      // Mount workspace
      '-v',
      `${workDir}:/workspace`,
      // Mount Docker socket for Docker-in-Docker (e2e tests need docker compose)
      '-v',
      '/var/run/docker.sock:/var/run/docker.sock',
      // Add node user to host's docker group (fixes permission denied)
      // CRITICAL: Without this, agent can't run docker commands inside container
      '--group-add',
      this._getDockerGid(),
      // Mount fresh Claude config to node user's home (read-write - Claude CLI writes settings, todos, etc.)
      '-v',
      `${clusterConfigDir}:/home/node/.claude`,
    ];

    // Add optional volume mounts (skip if path doesn't exist or isn't mountable)
    // Each mount is [hostPath, containerPath, options?]
    const optionalMounts = [
      [this._getGhConfigDir(), '/home/node/.config/gh', null], // gh credentials (read-write)
      [this._getGitConfigPath(), '/home/node/.gitconfig', 'ro'], // git config (read-only)
      [this._getAwsConfigDir(), '/home/node/.aws', 'ro'], // AWS credentials (read-only)
      [this._getKubeConfigDir(), '/home/node/.kube', 'ro'], // Kubernetes config (read-only)
      [this._getSshDir(), '/home/node/.ssh', 'ro'], // SSH keys (read-only)
      [this._getTerraformPluginDir(), '/home/node/.terraform.d', null], // Terraform cache (read-write)
    ];

    for (const [hostPath, containerPath, options] of optionalMounts) {
      if (hostPath && fs.existsSync(hostPath)) {
        const mountSpec = options ? `${hostPath}:${containerPath}:${options}` : `${hostPath}:${containerPath}`;
        args.push('-v', mountSpec);
      }
    }

    // Environment variables and final args
    args.push(
      '-e',
      `AWS_REGION=${process.env.AWS_REGION || 'eu-north-1'}`,
      '-e',
      `AWS_PROFILE=${process.env.AWS_PROFILE || 'default'}`,
      '-e',
      'AWS_PAGER=',
      '-w',
      '/workspace',
      image,
      'tail',
      '-f',
      '/dev/null'
    );

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
        if (code === 0) {
          const containerId = stdout.trim().substring(0, 12);
          this.containers.set(clusterId, containerId);

          // Install dependencies if package.json exists
          // This enables e2e tests and other npm-based tools to run
          // OPTIMIZATION: Skip npm install if node_modules already exists (30-40% faster startup)
          // See: GitHub issue #20
          try {
            console.log(`[IsolationManager] Checking for package.json in ${workDir}...`);
            if (fs.existsSync(path.join(workDir, 'package.json'))) {
              // Check if node_modules already exists in container (pre-baked or previous run)
              const checkResult = await this.execInContainer(
                clusterId,
                ['sh', '-c', 'test -d node_modules && test -f node_modules/.package-lock.json && echo "exists"'],
                {}
              );

              if (checkResult.code === 0 && checkResult.stdout.trim() === 'exists') {
                console.log(`[IsolationManager] ✓ Dependencies already installed (skipping npm install)`);
              } else {
                // Check if npm is available in container
                const npmCheck = await this.execInContainer(clusterId, ['which', 'npm'], {});
                if (npmCheck.code !== 0) {
                  console.log(`[IsolationManager] npm not available in container, skipping dependency install`);
                } else {
                  console.log(`[IsolationManager] Installing npm dependencies in container...`);

                  // Retry npm install with exponential backoff (network issues are common)
                  const maxRetries = 3;
                  const baseDelay = 2000; // 2 seconds
                  let installResult = null;

                  for (let attempt = 1; attempt <= maxRetries; attempt++) {
                    try {
                      installResult = await this.execInContainer(
                        clusterId,
                        ['sh', '-c', 'npm_config_engine_strict=false npm install --no-audit --no-fund'],
                        {}
                      );

                      if (installResult.code === 0) {
                        console.log(`[IsolationManager] ✓ Dependencies installed`);
                        break; // Success - exit retry loop
                      }

                      // Failed - retry if not last attempt
                      // Use stderr if available, otherwise stdout (npm writes some errors to stdout)
                      const errorOutput = (installResult.stderr || installResult.stdout || '').slice(0, 500);
                      if (attempt < maxRetries) {
                        const delay = baseDelay * Math.pow(2, attempt - 1);
                        console.warn(
                          `[IsolationManager] ⚠️ npm install failed (attempt ${attempt}/${maxRetries}), retrying in ${delay}ms...`
                        );
                        console.warn(`[IsolationManager] Error: ${errorOutput}`);
                        await new Promise((_resolve) => setTimeout(_resolve, delay));
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
                        await new Promise((_resolve) => setTimeout(_resolve, delay));
                      } else {
                        throw execErr; // Re-throw on last attempt
                      }
                    }
                  }
                }
              }
            }
          } catch (err) {
            console.warn(
              `[IsolationManager] ⚠️ Failed to install dependencies (non-fatal): ${err.message}`
            );
          }

          resolve(containerId);
        } else {
          reject(new Error(`Failed to create container: ${stderr}`));
        }
      });

      proc.on('error', (err) => {
        reject(new Error(`Docker spawn error: ${err.message}`));
      });
    });
  }

  /**
   * Execute a command inside the container
   * @param {string} clusterId - Cluster ID
   * @param {string[]} command - Command and arguments
   * @param {object} [options] - Exec options
   * @param {boolean} [options.interactive] - Use -it flags
   * @param {object} [options.env] - Environment variables
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

    return new Promise((resolve, reject) => {
      const proc = spawn('docker', args, {
        stdio: options.interactive ? 'inherit' : ['pipe', 'pipe', 'pipe'],
      });

      let stdout = '';
      let stderr = '';

      if (!options.interactive) {
        proc.stdout.on('data', (data) => {
          stdout += data;
        });
        proc.stderr.on('data', (data) => {
          stderr += data;
        });
      }

      proc.on('close', (code) => {
        resolve({ stdout, stderr, code });
      });

      proc.on('error', (err) => {
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
        console.log(`[IsolationManager] Preserving isolated workspace at ${isolatedInfo.path} for resume`);
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
   * @returns {string} Path to isolated directory
   */
  _createIsolatedCopy(clusterId, sourceDir) {
    const isolatedPath = path.join(os.tmpdir(), 'zeroshot-isolated', clusterId);

    // Clean up existing dir
    if (fs.existsSync(isolatedPath)) {
      fs.rmSync(isolatedPath, { recursive: true, force: true });
    }

    // Create directory
    fs.mkdirSync(isolatedPath, { recursive: true });

    // Copy files (excluding .git and common build artifacts)
    this._copyDirExcluding(sourceDir, isolatedPath, [
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
   */
  _copyDirExcluding(src, dest, exclude) {
    // Phase 1: Collect all files and directories
    const files = [];
    const directories = new Set();

    const collectFiles = (currentSrc, relativePath = '') => {
      let entries;
      try {
        entries = fs.readdirSync(currentSrc, { withFileTypes: true });
      } catch (err) {
        if (err.code === 'EACCES' || err.code === 'EPERM' || err.code === 'ENOENT') {
          return;
        }
        throw err;
      }

      for (const entry of entries) {
        // Check exclusions (exact match or glob pattern)
        const shouldExclude = exclude.some((pattern) => {
          if (pattern.startsWith('*.')) {
            return entry.name.endsWith(pattern.slice(1));
          }
          return entry.name === pattern;
        });
        if (shouldExclude) continue;

        const srcPath = path.join(currentSrc, entry.name);
        const relPath = relativePath ? path.join(relativePath, entry.name) : entry.name;

        try {
          // Handle symlinks: resolve to actual target
          if (entry.isSymbolicLink()) {
            const targetStats = fs.statSync(srcPath);
            if (targetStats.isDirectory()) {
              directories.add(relPath);
              collectFiles(srcPath, relPath);
            } else {
              files.push(relPath);
              // Ensure parent directory is tracked
              if (relativePath) directories.add(relativePath);
            }
          } else if (entry.isDirectory()) {
            directories.add(relPath);
            collectFiles(srcPath, relPath);
          } else {
            files.push(relPath);
            // Ensure parent directory is tracked
            if (relativePath) directories.add(relativePath);
          }
        } catch (err) {
          if (err.code === 'EACCES' || err.code === 'EPERM' || err.code === 'ENOENT') {
            continue;
          }
          throw err;
        }
      }
    };

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

    // Wait for all workers synchronously (required for this sync API)
    // We use a busy-wait pattern since _copyDirExcluding is called synchronously
    let completed = false;
    let workerError = null;

    Promise.all(workerPromises)
      .then(() => {
        completed = true;
      })
      .catch((err) => {
        workerError = err;
        completed = true;
      });

    // Busy wait for workers to complete
    // This is acceptable since it's still faster than sequential copy for large repos
    const startTime = Date.now();
    const timeout = 300000; // 5 minute timeout
    while (!completed) {
      if (Date.now() - startTime > timeout) {
        throw new Error('Parallel copy timed out after 5 minutes');
      }
      // Small sleep to prevent CPU spinning
      const waitUntil = Date.now() + 10;
      while (Date.now() < waitUntil) {
        // spin
      }
    }

    if (workerError) {
      throw workerError;
    }
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
   * @returns {string} Path to cluster-specific config directory
   */
  _createClusterConfigDir(clusterId) {
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
    const settings = {
      hooks: {
        PreToolUse: [
          {
            matcher: 'AskUserQuestion',
            hooks: [
              {
                type: 'command',
                command: '/home/node/.claude/hooks/block-ask-user-question.py',
              },
            ],
          },
        ],
      },
    };
    fs.writeFileSync(path.join(configDir, 'settings.json'), JSON.stringify(settings, null, 2));

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

    let foundState = false;

    for (const checkDir of checkDirs) {
      if (!fs.existsSync(checkDir)) continue;

      const hasStateFiles = stateFiles.some((file) => fs.existsSync(path.join(checkDir, file)));

      if (hasStateFiles) {
        const stateDir = path.join(os.homedir(), '.zeroshot', 'terraform-state', clusterId);
        fs.mkdirSync(stateDir, { recursive: true });

        for (const file of stateFiles) {
          const srcPath = path.join(checkDir, file);
          if (fs.existsSync(srcPath)) {
            const destPath = path.join(stateDir, file);
            try {
              fs.copyFileSync(srcPath, destPath);
              console.log(`[IsolationManager] Preserved Terraform state: ${file} → ${stateDir}`);
              foundState = true;
            } catch (err) {
              console.warn(`[IsolationManager] Failed to preserve ${file}: ${err.message}`);
            }
          }
        }
        break; // Only backup from first dir with state files
      }
    }

    if (!foundState) {
      console.log(`[IsolationManager] No Terraform state found to preserve`);
    }
  }

  /**
   * Get AWS config directory
   * @private
   */
  _getAwsConfigDir() {
    return process.env.AWS_CONFIG_DIR || path.join(os.homedir(), '.aws');
  }

  /**
   * Get Kubernetes config directory
   * @private
   */
  _getKubeConfigDir() {
    return process.env.KUBECONFIG_DIR || path.join(os.homedir(), '.kube');
  }

  /**
   * Get SSH directory
   * @private
   */
  _getSshDir() {
    return path.join(os.homedir(), '.ssh');
  }

  /**
   * Get Terraform plugin cache directory
   * @private
   */
  _getTerraformPluginDir() {
    const dir = path.join(os.homedir(), '.terraform.d');
    if (!fs.existsSync(dir)) fs.mkdirSync(dir, { recursive: true });
    return dir;
  }

  /**
   * Get gh CLI config directory (for PR creation)
   * @private
   */
  _getGhConfigDir() {
    return path.join(os.homedir(), '.config', 'gh');
  }

  /**
   * Get git config file path (for commit identity)
   * Returns null if .gitconfig doesn't exist or is a directory (e.g., on GitHub Actions)
   * @private
   */
  _getGitConfigPath() {
    const gitConfigPath = path.join(os.homedir(), '.gitconfig');
    try {
      const stat = fs.statSync(gitConfigPath);
      if (stat.isFile()) {
        return gitConfigPath;
      }
      // .gitconfig exists but is a directory (GitHub Actions runner has this issue)
      return null;
    } catch {
      // .gitconfig doesn't exist
      return null;
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
      const result = execSync(`docker inspect -f '{{.State.Running}}' ${containerId} 2>/dev/null`, {
        encoding: 'utf8',
      });
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
      execSync(`docker rm -f ${name} 2>/dev/null`, { encoding: 'utf8' });
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
      execSync('docker --version', { encoding: 'utf8', stdio: 'pipe' });
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
      execSync(`docker image inspect ${image} 2>/dev/null`, {
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
        execSync(`docker build -f docker/zeroshot-cluster/Dockerfile -t ${image} .`, {
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
   * Create a git worktree for isolated work
   * @private
   * @param {string} clusterId - Cluster ID (used as branch name)
   * @param {string} workDir - Original working directory
   * @returns {{ path: string, branch: string, repoRoot: string }}
   */
  _createWorktree(clusterId, workDir) {
    const repoRoot = this._getGitRoot(workDir);
    if (!repoRoot) {
      throw new Error(`Cannot find git root for ${workDir}`);
    }

    // Create branch name from cluster ID (e.g., cluster-cosmic-meteor-87 -> zeroshot/cosmic-meteor-87)
    const branchName = `zeroshot/${clusterId.replace(/^cluster-/, '')}`;

    // Worktree path in tmp
    const worktreePath = path.join(os.tmpdir(), 'zeroshot-worktrees', clusterId);

    // Ensure parent directory exists
    const parentDir = path.dirname(worktreePath);
    if (!fs.existsSync(parentDir)) {
      fs.mkdirSync(parentDir, { recursive: true });
    }

    // Remove existing worktree if it exists (cleanup from previous run)
    try {
      execSync(`git worktree remove --force "${worktreePath}" 2>/dev/null`, {
        cwd: repoRoot,
        encoding: 'utf8',
        stdio: 'pipe',
      });
    } catch {
      // Ignore - worktree doesn't exist
    }

    // Delete the branch if it exists (from previous run)
    try {
      execSync(`git branch -D "${branchName}" 2>/dev/null`, {
        cwd: repoRoot,
        encoding: 'utf8',
        stdio: 'pipe',
      });
    } catch {
      // Ignore - branch doesn't exist
    }

    // Create worktree with new branch based on HEAD
    execSync(`git worktree add -b "${branchName}" "${worktreePath}" HEAD`, {
      cwd: repoRoot,
      encoding: 'utf8',
      stdio: 'pipe',
    });

    return {
      path: worktreePath,
      branch: branchName,
      repoRoot,
    };
  }

  /**
   * Remove a git worktree
   * @private
   * @param {{ path: string, branch: string, repoRoot: string }} worktreeInfo
   */
  _removeWorktree(worktreeInfo) {
    try {
      // Remove the worktree
      execSync(`git worktree remove --force "${worktreeInfo.path}" 2>/dev/null`, {
        cwd: worktreeInfo.repoRoot,
        encoding: 'utf8',
        stdio: 'pipe',
      });
    } catch {
      // Fallback: manually remove directory if worktree command fails
      try {
        fs.rmSync(worktreeInfo.path, { recursive: true, force: true });
      } catch {
        // Ignore
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
