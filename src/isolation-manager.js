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
const path = require('path');
const os = require('os');
const fs = require('fs');

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
   * @returns {Promise<string>} Container ID
   */
  createContainer(clusterId, config) {
    const image = config.image || this.image;
    let workDir = config.workDir || process.cwd();
    const containerName = `zeroshot-cluster-${clusterId}`;

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
    if (this._isGitRepo(workDir)) {
      const isolatedDir = this._createIsolatedCopy(clusterId, workDir);
      this.isolatedDirs = this.isolatedDirs || new Map();
      this.isolatedDirs.set(clusterId, {
        path: isolatedDir,
        originalDir: workDir,
      });
      workDir = isolatedDir;
      console.log(`[IsolationManager] Created isolated copy at ${workDir}`);
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
      // Mount gh credentials (read-write - gh auth setup-git needs to write)
      '-v',
      `${this._getGhConfigDir()}:/home/node/.config/gh`,
      // Mount git config (read-only - for git identity)
      '-v',
      `${this._getGitConfigPath()}:/home/node/.gitconfig:ro`,
      // Mount AWS credentials (read-only)
      '-v',
      `${this._getAwsConfigDir()}:/home/node/.aws:ro`,
      // Mount Kubernetes config (read-only)
      '-v',
      `${this._getKubeConfigDir()}:/home/node/.kube:ro`,
      // Mount SSH keys (read-only)
      '-v',
      `${this._getSshDir()}:/home/node/.ssh:ro`,
      // Mount Terraform plugin cache (read-write for caching)
      '-v',
      `${this._getTerraformPluginDir()}:/home/node/.terraform.d`,
      // Environment variables for infrastructure tasks
      '-e',
      `AWS_REGION=${process.env.AWS_REGION || 'eu-north-1'}`,
      '-e',
      `AWS_PROFILE=${process.env.AWS_PROFILE || 'default'}`,
      '-e',
      'AWS_PAGER=',
      // Set working directory
      '-w',
      '/workspace',
      // Keep container running
      image,
      'tail',
      '-f',
      '/dev/null',
    ];

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
          try {
            console.log(`[IsolationManager] Checking for package.json in ${workDir}...`);
            if (fs.existsSync(path.join(workDir, 'package.json'))) {
              console.log(`[IsolationManager] Installing npm dependencies in container...`);

              // Retry npm install with exponential backoff (network issues are common)
              const maxRetries = 3;
              const baseDelay = 2000; // 2 seconds
              let installResult = null;

              for (let attempt = 1; attempt <= maxRetries; attempt++) {
                try {
                  installResult = await this.execInContainer(
                    clusterId,
                    ['sh', '-c', 'npm install --no-audit --no-fund 2>&1'],
                    {}
                  );

                  if (installResult.code === 0) {
                    console.log(`[IsolationManager] ✓ Dependencies installed`);
                    break; // Success - exit retry loop
                  }

                  // Failed - retry if not last attempt
                  if (attempt < maxRetries) {
                    const delay = baseDelay * Math.pow(2, attempt - 1);
                    console.warn(
                      `[IsolationManager] ⚠️ npm install failed (attempt ${attempt}/${maxRetries}), retrying in ${delay}ms...`
                    );
                    console.warn(`[IsolationManager] Error: ${installResult.stderr.slice(0, 200)}`);
                    await new Promise((_resolve) => setTimeout(_resolve, delay));
                  } else {
                    console.warn(
                      `[IsolationManager] ⚠️ npm install failed after ${maxRetries} attempts (non-fatal): ${installResult.stderr.slice(0, 200)}`
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
  stopContainer(clusterId, timeout = 10) {
    const containerId = this.containers.get(clusterId);
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
  removeContainer(clusterId, force = false) {
    const containerId = this.containers.get(clusterId);
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
   * Stop and remove a container, and clean up isolated dir/config
   * @param {string} clusterId - Cluster ID
   * @returns {Promise<void>}
   */
  async cleanup(clusterId) {
    await this.stopContainer(clusterId);
    await this.removeContainer(clusterId);

    // Clean up isolated directory if one was created
    if (this.isolatedDirs?.has(clusterId)) {
      const isolatedInfo = this.isolatedDirs.get(clusterId);
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

    // Clean up cluster config dir
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

    // Initialize fresh git repo
    execSync('git init', { cwd: isolatedPath, stdio: 'pipe' });

    // Add remote if source had one (needed for git push / PR creation)
    // Inject gh token into URL for authentication inside container
    if (remoteUrl) {
      let authRemoteUrl = remoteUrl;
      const token = this._getGhToken();
      if (token && remoteUrl.startsWith('https://github.com/')) {
        // Convert https://github.com/org/repo.git to https://x-access-token:TOKEN@github.com/org/repo.git
        authRemoteUrl = remoteUrl.replace(
          'https://github.com/',
          `https://x-access-token:${token}@github.com/`
        );
      }
      execSync(`git remote add origin "${authRemoteUrl}"`, {
        cwd: isolatedPath,
        stdio: 'pipe',
      });
    }

    execSync('git add -A', { cwd: isolatedPath, stdio: 'pipe' });

    try {
      execSync('git commit -m "Initial commit (isolated copy)"', {
        cwd: isolatedPath,
        stdio: 'pipe',
      });
    } catch {
      // May fail if nothing to commit (empty dir)
    }

    // Create feature branch for work
    const branchName = `zeroshot/${clusterId}`;
    execSync(`git checkout -b "${branchName}"`, {
      cwd: isolatedPath,
      stdio: 'pipe',
    });

    return isolatedPath;
  }

  /**
   * Copy directory excluding certain paths
   * Supports exact matches and glob patterns (*.ext)
   * @private
   */
  _copyDirExcluding(src, dest, exclude) {
    const entries = fs.readdirSync(src, { withFileTypes: true });

    for (const entry of entries) {
      // Check exclusions (exact match or glob pattern)
      const shouldExclude = exclude.some((pattern) => {
        if (pattern.startsWith('*.')) {
          return entry.name.endsWith(pattern.slice(1));
        }
        return entry.name === pattern;
      });
      if (shouldExclude) continue;

      const srcPath = path.join(src, entry.name);
      const destPath = path.join(dest, entry.name);

      try {
        // Handle symlinks: resolve to actual target and copy appropriately
        // This avoids EISDIR errors when symlink points to directory
        if (entry.isSymbolicLink()) {
          // Get the actual target stats (follows the symlink)
          const targetStats = fs.statSync(srcPath);
          if (targetStats.isDirectory()) {
            fs.mkdirSync(destPath, { recursive: true });
            this._copyDirExcluding(srcPath, destPath, exclude);
          } else {
            fs.copyFileSync(srcPath, destPath);
          }
        } else if (entry.isDirectory()) {
          fs.mkdirSync(destPath, { recursive: true });
          this._copyDirExcluding(srcPath, destPath, exclude);
        } else {
          fs.copyFileSync(srcPath, destPath);
        }
      } catch (err) {
        // Skip files we can't copy (permission denied, broken symlinks, etc.)
        // These are usually cache/temp files that aren't needed
        if (err.code === 'EACCES' || err.code === 'EPERM' || err.code === 'ENOENT') {
          continue;
        }
        throw err; // Re-throw other errors
      }
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

    // Create fresh directory and hooks subdirectory
    fs.mkdirSync(configDir, { recursive: true });
    const hooksDir = path.join(configDir, 'hooks');
    fs.mkdirSync(hooksDir, { recursive: true });

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
   * @private
   */
  _getGitConfigPath() {
    return path.join(os.homedir(), '.gitconfig');
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
    const dockerfilePath = path.join(__dirname, '..', 'docker', 'zeroshot-cluster');

    if (!fs.existsSync(path.join(dockerfilePath, 'Dockerfile'))) {
      throw new Error(`Dockerfile not found at ${dockerfilePath}/Dockerfile`);
    }

    console.log(`[IsolationManager] Building Docker image '${image}'...`);

    const baseDelay = 3000; // 3 seconds

    for (let attempt = 1; attempt <= maxRetries; attempt++) {
      try {
        // Use execSync with stdio: 'inherit' to stream output in real-time
        execSync(`docker build -t ${image} .`, {
          cwd: dockerfilePath,
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
