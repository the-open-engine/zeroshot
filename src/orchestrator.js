/**
 * Orchestrator - Manages cluster lifecycle
 *
 * Provides:
 * - Cluster initialization and configuration
 * - Agent lifecycle management
 * - GitHub issue integration
 * - Cluster state tracking
 * - Crash recovery
 */

const fs = require('fs');
const path = require('path');
const os = require('os');
const lockfile = require('proper-lockfile');

// Stale lock timeout in ms - if lock file is older than this, delete it
const LOCK_STALE_MS = 5000;

/**
 * Remove lock file if it's stale (older than LOCK_STALE_MS)
 * Handles crashes that leave orphaned lock files
 */
function cleanStaleLock(lockPath) {
  try {
    if (fs.existsSync(lockPath)) {
      const age = Date.now() - fs.statSync(lockPath).mtimeMs;
      if (age > LOCK_STALE_MS) {
        fs.unlinkSync(lockPath);
      }
    }
  } catch {
    // Ignore - another process may have cleaned it
  }
}
const AgentWrapper = require('./agent-wrapper');
const SubClusterWrapper = require('./sub-cluster-wrapper');
const MessageBus = require('./message-bus');
const Ledger = require('./ledger');
const InputHelpers = require('./input-helpers');
const { detectProvider } = require('./issue-providers');
const IsolationManager = require('./isolation-manager');
const { generateName } = require('./name-generator');
const configValidator = require('./config-validator');
const TemplateResolver = require('./template-resolver');
const { loadSettings } = require('../lib/settings');
const { normalizeProviderName } = require('../lib/provider-names');
const StateSnapshotter = require('./state-snapshotter');
const crypto = require('crypto');

function applyModelOverride(agentConfig, modelOverride) {
  if (!modelOverride) return;

  agentConfig.model = modelOverride;
  if (agentConfig.modelRules) {
    delete agentConfig.modelRules;
  }
  if (agentConfig.modelConfig) {
    delete agentConfig.modelConfig;
  }
}

/**
 * Operation Chain Schema
 * Conductor (or any agent) can publish CLUSTER_OPERATIONS to dynamically modify cluster
 *
 * Supported operations:
 * - add_agents: Spawn new agents with given configs
 * - remove_agents: Stop and remove agents by ID
 * - update_agent: Modify existing agent config
 * - publish: Publish a message to the bus
 * - load_config: Load agents from a named cluster config template
 */
const VALID_OPERATIONS = ['add_agents', 'remove_agents', 'update_agent', 'publish', 'load_config'];

/**
 * Workflow-triggering topics that indicate cluster state progression
 * These are the topics that MATTER for resume - not AGENT_OUTPUT noise
 */
const WORKFLOW_TRIGGERS = Object.freeze([
  'ISSUE_OPENED',
  'PLAN_READY',
  'IMPLEMENTATION_READY',
  'VALIDATION_RESULT',
  'CONDUCTOR_ESCALATE',
]);

class Orchestrator {
  constructor(options = {}) {
    this.clusters = new Map(); // cluster_id -> cluster object
    this.quiet = options.quiet || false; // Suppress verbose logging

    // TaskRunner DI - allows injecting MockTaskRunner for testing
    // When set, passed to all AgentWrappers to control task execution
    this.taskRunner = options.taskRunner || null;

    // Set up persistent storage directory (can be overridden for testing)
    this.storageDir = options.storageDir || path.join(os.homedir(), '.zeroshot');
    if (!fs.existsSync(this.storageDir)) {
      fs.mkdirSync(this.storageDir, { recursive: true });
    }

    // Track if orchestrator is closed (prevents _saveClusters race conditions during cleanup)
    this.closed = false;

    // Track if clusters are loaded (for lazy loading pattern)
    this._clustersLoaded = options.skipLoad === true;
  }

  /**
   * Factory method for async initialization
   * Use this instead of `new Orchestrator()` for proper async cluster loading
   * @param {Object} options - Same options as constructor
   * @returns {Promise<Orchestrator>}
   */
  static async create(options = {}) {
    const instance = new Orchestrator({ ...options, skipLoad: true });
    if (options.skipLoad !== true) {
      await instance._loadClusters();
      instance._clustersLoaded = true;
    }
    return instance;
  }

  /**
   * Log message (respects quiet mode)
   * @private
   */
  _log(...args) {
    if (!this.quiet) {
      console.log(...args);
    }
  }

  /**
   * Get input source type for metadata
   * @param {Object} input - Input object
   * @returns {string} Source type: 'github', 'file', or 'text'
   * @private
   */
  _getInputSource(input) {
    if (input.issue) return 'github';
    if (input.file) return 'file';
    return 'text';
  }

  /**
   * Load clusters from persistent storage
   * Uses file locking for consistent reads
   * @private
   */
  async _loadClusters() {
    const clustersFile = path.join(this.storageDir, 'clusters.json');
    this._log(`[Orchestrator] Loading clusters from: ${clustersFile}`);

    if (!fs.existsSync(clustersFile)) {
      this._log(`[Orchestrator] No clusters file found at ${clustersFile}`);
      return;
    }

    const lockfilePath = path.join(this.storageDir, 'clusters.json.lock');
    let release;

    try {
      // Clean stale locks from crashed processes
      cleanStaleLock(lockfilePath);

      // Acquire lock with async API (proper retries without CPU spin-wait)
      release = await lockfile.lock(clustersFile, {
        lockfilePath,
        stale: LOCK_STALE_MS,
        retries: {
          retries: 20,
          minTimeout: 100,
          maxTimeout: 200,
          randomize: true,
        },
      });

      const data = JSON.parse(fs.readFileSync(clustersFile, 'utf8'));
      const clusterIds = Object.keys(data);
      this._log(`[Orchestrator] Found ${clusterIds.length} clusters in file:`, clusterIds);

      // Track clusters to remove (missing .db files or 0 messages)
      const clustersToRemove = [];
      // Track clusters with 0 messages (corrupted from SIGINT race condition)
      const corruptedClusters = [];

      for (const [clusterId, clusterData] of Object.entries(data)) {
        // Skip clusters whose .db file doesn't exist (orphaned registry entries)
        const dbPath = path.join(this.storageDir, `${clusterId}.db`);
        if (!fs.existsSync(dbPath)) {
          console.warn(
            `[Orchestrator] Cluster ${clusterId} has no database file, removing from registry`
          );
          clustersToRemove.push(clusterId);
          continue;
        }

        this._log(`[Orchestrator] Loading cluster: ${clusterId}`);
        const cluster = this._loadSingleCluster(clusterId, clusterData);

        // VALIDATION: Detect 0-message clusters (corrupted from SIGINT during initialization)
        // These clusters were created before the initCompletePromise fix was applied
        if (cluster && cluster.messageBus) {
          const messageCount = cluster.messageBus.count({ cluster_id: clusterId });
          if (messageCount === 0) {
            console.warn(`[Orchestrator] ⚠️  Cluster ${clusterId} has 0 messages (corrupted)`);
            console.warn(
              `[Orchestrator]    This likely occurred from SIGINT during initialization.`
            );
            console.warn(
              `[Orchestrator]    Marking as 'corrupted' - use 'zeroshot kill ${clusterId}' to remove.`
            );
            corruptedClusters.push(clusterId);
            // Mark cluster as corrupted for visibility in status/list commands
            cluster.state = 'corrupted';
            cluster.corruptedReason = 'SIGINT during initialization (0 messages in ledger)';
          }
        }
      }

      // Clean up orphaned entries from clusters.json
      if (clustersToRemove.length > 0) {
        for (const clusterId of clustersToRemove) {
          delete data[clusterId];
        }
        fs.writeFileSync(clustersFile, JSON.stringify(data, null, 2));
        this._log(
          `[Orchestrator] Removed ${clustersToRemove.length} orphaned cluster(s) from registry`
        );
      }

      // Log summary of corrupted clusters
      if (corruptedClusters.length > 0) {
        console.warn(
          `\n[Orchestrator] ⚠️  Found ${corruptedClusters.length} corrupted cluster(s):`
        );
        for (const clusterId of corruptedClusters) {
          console.warn(`    - ${clusterId}`);
        }
        console.warn(`[Orchestrator] Run 'zeroshot clear' to remove all corrupted clusters.\n`);
      }

      this._log(`[Orchestrator] Total clusters loaded: ${this.clusters.size}`);
    } catch (error) {
      console.error('[Orchestrator] Failed to load clusters:', error.message);
      console.error(error.stack);
    } finally {
      if (release) {
        await release();
      }
    }
  }

  /**
   * Load a single cluster from data
   * @private
   */
  _loadSingleCluster(clusterId, clusterData) {
    // Skip if already loaded
    if (this.clusters.has(clusterId)) {
      return this.clusters.get(clusterId);
    }

    // Restore ledger and message bus
    const dbPath = path.join(this.storageDir, `${clusterId}.db`);
    const ledger = new Ledger(dbPath);
    const messageBus = new MessageBus(ledger);

    // Restore isolation manager FIRST if cluster was running in isolation mode
    const { isolation, isolationManager } = this._restoreClusterIsolation(clusterId, clusterData);

    // Reconstruct agent metadata from config (processes are ephemeral)
    // CRITICAL: Pass isolation context to agents if cluster was running in isolation
    const agents = this._rebuildClusterAgents(
      clusterId,
      clusterData,
      messageBus,
      isolation,
      isolationManager
    );

    const cluster = {
      ...clusterData,
      ledger,
      messageBus,
      agents,
      isolation,
      autoPr: clusterData.autoPr || false,
    };

    this.clusters.set(clusterId, cluster);
    this._startSnapshotter(cluster);
    this._log(`[Orchestrator] Loaded cluster: ${clusterId} with ${agents.length} agents`);

    return cluster;
  }

  _restoreClusterIsolation(clusterId, clusterData) {
    let isolation = clusterData.isolation || null;
    let isolationManager = null;

    if (isolation?.enabled && isolation.containerId) {
      isolationManager = new IsolationManager({ image: isolation.image });
      // Restore the container mapping so cleanup works
      isolationManager.containers.set(clusterId, isolation.containerId);
      // Restore isolated dir mapping for workspace preservation during cleanup
      if (isolation.workDir) {
        isolationManager.isolatedDirs.set(clusterId, {
          path: path.join(os.tmpdir(), 'zeroshot-isolated', clusterId),
          originalDir: isolation.workDir,
        });
      }
      isolation = {
        ...isolation,
        manager: isolationManager,
      };
      this._log(
        `[Orchestrator] Restored isolation manager for ${clusterId} (container: ${isolation.containerId}, workDir: ${isolation.workDir || 'unknown'})`
      );
    }

    return { isolation, isolationManager };
  }

  _resolveAgentCwd(clusterData) {
    const worktreePath = clusterData.worktree?.path;
    const isolationWorkDir = clusterData.isolation?.workDir;
    return worktreePath || isolationWorkDir || null;
  }

  _buildAgentOptions(clusterId, clusterData, isolation, isolationManager) {
    const agentOptions = {
      id: clusterId,
      quiet: this.quiet,
      modelOverride: clusterData.modelOverride || null,
    };

    if (isolation?.enabled && isolationManager) {
      agentOptions.isolation = {
        enabled: true,
        manager: isolationManager,
        clusterId,
      };
    }

    return agentOptions;
  }

  _instantiateAgent(agentConfig, messageBus, clusterId, agentOptions) {
    if (agentConfig.type === 'subcluster') {
      return new SubClusterWrapper(agentConfig, messageBus, { id: clusterId }, agentOptions);
    }

    return new AgentWrapper(agentConfig, messageBus, { id: clusterId }, agentOptions);
  }

  _restoreAgentState(agent, agentConfig, clusterData) {
    if (!clusterData.agentStates) return;

    const savedState = clusterData.agentStates.find((state) => state.id === agentConfig.id);
    if (!savedState) return;

    agent.state = savedState.state || 'idle';
    agent.iteration = savedState.iteration || 0;
    agent.currentTask = savedState.currentTask || false;
    agent.currentTaskId = savedState.currentTaskId || null;
    agent.processPid = savedState.processPid || null;
  }

  _rebuildClusterAgents(clusterId, clusterData, messageBus, isolation, isolationManager) {
    const agents = [];
    const agentCwd = this._resolveAgentCwd(clusterData);

    if (!clusterData.config?.agents) {
      return agents;
    }

    for (const agentConfig of clusterData.config.agents) {
      if (!agentConfig.cwd && agentCwd) {
        agentConfig.cwd = agentCwd;
        this._log(`[Orchestrator] Fixed missing cwd for agent ${agentConfig.id}: ${agentCwd}`);
      }

      if (clusterData.modelOverride) {
        applyModelOverride(agentConfig, clusterData.modelOverride);
      }

      const agentOptions = this._buildAgentOptions(
        clusterId,
        clusterData,
        isolation,
        isolationManager
      );
      const agent = this._instantiateAgent(agentConfig, messageBus, clusterId, agentOptions);
      this._restoreAgentState(agent, agentConfig, clusterData);
      agents.push(agent);
    }

    return agents;
  }

  _startSnapshotter(cluster) {
    if (cluster.snapshotter) {
      cluster.snapshotter.start();
      return;
    }

    const snapshotter = new StateSnapshotter({
      messageBus: cluster.messageBus,
      clusterId: cluster.id,
    });
    snapshotter.start();
    cluster.snapshotter = snapshotter;
  }

  /**
   * Ensure clusters file exists (required for file locking)
   * @private
   */
  _ensureClustersFile() {
    if (!fs.existsSync(this.storageDir)) {
      try {
        fs.mkdirSync(this.storageDir, { recursive: true });
      } catch (error) {
        console.warn(
          `[Orchestrator] Failed to create storage directory ${this.storageDir}: ${error.message}`
        );
        return null;
      }
    }

    const clustersFile = path.join(this.storageDir, 'clusters.json');
    if (!fs.existsSync(clustersFile)) {
      fs.writeFileSync(clustersFile, '{}');
    }
    return clustersFile;
  }

  /**
   * Save clusters to persistent storage
   * Uses file locking to prevent race conditions with other processes
   * @private
   */
  async _saveClusters() {
    // Skip saving if orchestrator is closed (prevents race conditions during cleanup)
    if (this.closed) {
      return;
    }

    const clustersFile = this._ensureClustersFile();
    if (!clustersFile) {
      return;
    }
    const lockfilePath = path.join(this.storageDir, 'clusters.json.lock');
    let release;

    try {
      // Clean stale locks from crashed processes
      cleanStaleLock(lockfilePath);

      // Acquire exclusive lock with async API (proper retries without CPU spin-wait)
      release = await lockfile.lock(clustersFile, {
        lockfilePath,
        stale: LOCK_STALE_MS,
        retries: {
          retries: 50,
          minTimeout: 100,
          maxTimeout: 300,
          randomize: true,
        },
      });

      // Read existing clusters from file (other processes may have added clusters)
      let existingClusters = {};
      try {
        const content = fs.readFileSync(clustersFile, 'utf8');
        existingClusters = JSON.parse(content);
      } catch (error) {
        console.error('[Orchestrator] Failed to read existing clusters:', error.message);
      }

      // Merge: update/add clusters from this process
      for (const [clusterId, cluster] of this.clusters.entries()) {
        // CRITICAL: Only update clusters this process actually owns or has modified
        // A process owns a cluster if: it started it (pid matches) OR it explicitly stopped/killed it
        const isOwnedByThisProcess = cluster.pid === process.pid;
        const wasModifiedByThisProcess = cluster.state === 'stopped' || cluster.state === 'killed';

        // Skip clusters we don't own and haven't modified - prevents race condition
        // where a running cluster overwrites another process's stop/kill operation
        if (!isOwnedByThisProcess && !wasModifiedByThisProcess) {
          // Preserve existing state from file for clusters we don't own
          continue;
        }

        // CRITICAL: Killed clusters are DELETED from disk, not persisted
        // This ensures they can't be accidentally resumed
        if (cluster.state === 'killed') {
          delete existingClusters[clusterId];
          continue;
        }

        existingClusters[clusterId] = {
          id: cluster.id,
          config: cluster.config,
          state: cluster.state,
          createdAt: cluster.createdAt,
          // Track PID for zombie detection (null if cluster is stopped/killed)
          pid: cluster.state === 'running' ? cluster.pid : null,
          // Persist failure info for resume capability
          failureInfo: cluster.failureInfo || null,
          // Persist PR mode for completion agent selection
          autoPr: cluster.autoPr || false,
          // Persist model override for consistent agent spawning on resume
          modelOverride: cluster.modelOverride || null,
          // Persist isolation info (excluding manager instance which can't be serialized)
          // CRITICAL: workDir is required for resume() to recreate container with same workspace
          isolation: cluster.isolation
            ? {
                enabled: cluster.isolation.enabled,
                containerId: cluster.isolation.containerId,
                image: cluster.isolation.image,
                workDir: cluster.isolation.workDir, // Required for resume
              }
            : null,
          // Persist agent runtime states for accurate status display from other processes
          agentStates: cluster.agents
            ? cluster.agents.map((a) => ({
                id: a.id,
                state: a.state,
                iteration: a.iteration,
                currentTask: a.currentTask ? true : false,
                currentTaskId: a.currentTaskId,
                processPid: a.processPid,
              }))
            : null,
        };
      }

      // Write merged data
      fs.writeFileSync(clustersFile, JSON.stringify(existingClusters, null, 2));
      this._log(
        `[Orchestrator] Saved ${this.clusters.size} cluster(s), file now has ${Object.keys(existingClusters).length} total`
      );
    } catch (error) {
      if (error.code === 'ENOENT') {
        console.warn(
          `[Orchestrator] Skipping cluster save; storage directory missing: ${this.storageDir}`
        );
        return;
      }
      throw error;
    } finally {
      // Always release lock
      if (release) {
        await release();
      }
    }
  }

  /**
   * Watch for new clusters and call callback when found
   * Polls the clusters file for changes with file locking
   * @param {Function} onNewCluster - Callback(cluster) for each new cluster
   * @param {Number} intervalMs - Poll interval in ms (default: 2000)
   * @returns {Function} Stop function to cancel watching
   */
  watchForNewClusters(onNewCluster, intervalMs = 2000) {
    const clustersFile = path.join(this.storageDir, 'clusters.json');
    const lockfilePath = path.join(this.storageDir, 'clusters.json.lock');
    const knownClusterIds = new Set(this.clusters.keys());

    const intervalId = setInterval(() => {
      let release;
      try {
        if (!fs.existsSync(clustersFile)) return;

        // Clean stale locks from crashed processes
        cleanStaleLock(lockfilePath);

        // Try to acquire lock once (polling is best-effort, will retry on next cycle)
        try {
          release = lockfile.lockSync(clustersFile, {
            lockfilePath,
            stale: LOCK_STALE_MS,
          });
        } catch (lockErr) {
          // Lock busy - skip this poll cycle, try again next interval
          if (lockErr.code === 'ELOCKED') return;
          throw lockErr;
        }

        const data = JSON.parse(fs.readFileSync(clustersFile, 'utf8'));

        for (const [clusterId, clusterData] of Object.entries(data)) {
          if (!knownClusterIds.has(clusterId)) {
            // New cluster found
            knownClusterIds.add(clusterId);
            const cluster = this._loadSingleCluster(clusterId, clusterData);
            if (cluster && onNewCluster) {
              onNewCluster(cluster);
            }
          }
        }
      } catch (error) {
        // File access during polling can fail transiently - log and continue
        console.error(`[Orchestrator] watchForNewClusters error (will retry): ${error.message}`);
      } finally {
        if (release) {
          release();
        }
      }
    }, intervalMs);

    // Return stop function
    return () => clearInterval(intervalId);
  }

  /**
   * Start a new cluster with mocked agent executors (TESTING ONLY)
   *
   * CRITICAL: This method PREVENTS real Claude API calls.
   * All agent behaviors must be defined in mockExecutor.
   *
   * @param {Object} config - Cluster configuration
   * @param {Object} input - Input source { issue, text, or config }
   * @param {MockAgentExecutor} mockExecutor - Mock executor with agent behaviors
   * @returns {Object} Cluster object
   */
  startWithMock(config, input, mockExecutor) {
    if (!mockExecutor) {
      throw new Error('Orchestrator.startWithMock: mockExecutor is required');
    }

    // Validate all agents that execute tasks have mock behaviors defined
    // Orchestrator agents (action: 'stop_cluster') don't execute tasks, so don't need mocks
    for (const agentConfig of config.agents) {
      const agentId = agentConfig.id;

      // Check if agent has any triggers that execute tasks
      const executesTask = agentConfig.triggers?.some(
        (trigger) => !trigger.action || trigger.action === 'execute_task'
      );

      if (executesTask && !mockExecutor.behaviors[agentId]) {
        throw new Error(
          `Orchestrator.startWithMock: No behavior defined for agent '${agentId}'. ` +
            `This would cause real Claude API calls. ABORTING.\n` +
            `Available behaviors: ${Object.keys(mockExecutor.behaviors).join(', ')}`
        );
      }
    }

    return this._startInternal(config, input, {
      mockExecutor,
      testMode: true,
    });
  }

  /**
   * Start a new cluster
   * @param {Object} config - Cluster configuration
   * @param {Object} input - Input source { issue, text, or config }
   * @param {Object} options - Start options
   * @param {boolean} options.isolation - Run in Docker container
   * @param {string} options.isolationImage - Docker image to use
   * @param {boolean} options.worktree - Run in git worktree isolation (lightweight, no Docker)
   * @returns {Object} Cluster object
   */
  start(config, input = {}, options = {}) {
    return this._startInternal(config, input, {
      testMode: false,
      cwd: options.cwd || process.cwd(), // Target working directory for agents
      isolation: options.isolation || false,
      isolationImage: options.isolationImage,
      worktree: options.worktree || false,
      autoPr: options.autoPr || process.env.ZEROSHOT_PR === '1',
      modelOverride: options.modelOverride, // Model override for all agents
      clusterId: options.clusterId, // Explicit ID from CLI/daemon parent
      settings: options.settings, // User settings for issue provider detection
      forceProvider: options.forceProvider, // Force specific issue provider
    });
  }

  /**
   * Internal start implementation (shared by start and startWithMock)
   * @private
   */
  async _startInternal(config, input = {}, options = {}) {
    // Generate a unique cluster ID for this process call.
    // IMPORTANT: Do NOT implicitly reuse ZEROSHOT_CLUSTER_ID, because:
    // - test harnesses may set it globally (breaking multi-start tests)
    // - callers may start multiple clusters in one process
    // Use it only when explicitly passed (CLI/daemon parent) via options.clusterId.
    const clusterId = this._generateUniqueClusterId(
      options.clusterId || null,
      config?.dbPath || null
    );

    // Create ledger and message bus with persistent storage
    const dbPath = config.dbPath || path.join(this.storageDir, `${clusterId}.db`);
    const ledger = new Ledger(dbPath);
    const messageBus = new MessageBus(ledger);

    // Handle isolation mode (Docker container OR git worktree)
    const { isolationManager, containerId, worktreeInfo } = await this._initializeIsolation(
      options,
      config,
      clusterId
    );

    // Build cluster object
    // CRITICAL: initComplete promise ensures ISSUE_OPENED is published before stop() completes
    // This prevents 0-message clusters from SIGINT during async initialization
    let resolveInitComplete;
    const initCompletePromise = new Promise((resolve) => {
      resolveInitComplete = resolve;
    });

    const cluster = {
      id: clusterId,
      config,
      state: 'initializing',
      messageBus,
      ledger,
      agents: [],
      createdAt: Date.now(),
      // Track PID for zombie detection (this process owns the cluster)
      pid: process.pid,
      // Initialization completion tracking (for safe SIGINT handling)
      initCompletePromise,
      _resolveInitComplete: resolveInitComplete,
      autoPr: options.autoPr || false,
      // Model override for all agents (applied to dynamically added agents)
      modelOverride: options.modelOverride || null,
      // Issue provider tracking (where issue was fetched from)
      issueProvider: null, // Set after fetching issue (github, gitlab, jira, azure-devops)
      // Git platform tracking (where PR/MR will be created - independent of issue provider)
      gitPlatform: null, // Set when --pr mode is active
      // Isolation state (only if enabled)
      // CRITICAL: Store workDir for resume capability - without this, resume() can't recreate container
      isolation: options.isolation
        ? {
            enabled: true,
            containerId,
            image: options.isolationImage || 'zeroshot-cluster-base',
            manager: isolationManager,
            workDir: options.cwd || process.cwd(), // Persisted for resume
          }
        : null,
      // Worktree isolation state (lightweight alternative to Docker)
      worktree: options.worktree
        ? {
            enabled: true,
            path: worktreeInfo.path,
            branch: worktreeInfo.branch,
            repoRoot: worktreeInfo.repoRoot,
            manager: isolationManager,
            workDir: options.cwd || process.cwd(), // Persisted for resume
          }
        : null,
    };

    this.clusters.set(clusterId, cluster);
    this._startSnapshotter(cluster);

    try {
      // Fetch input (issue from provider, file, or text)
      let inputData;
      if (input.issue) {
        // Detect provider and fetch issue
        const ProviderClass = detectProvider(
          input.issue,
          options.settings || {},
          options.forceProvider
        );
        if (!ProviderClass) {
          throw new Error(`No issue provider matched input: ${input.issue}`);
        }

        const provider = new ProviderClass();
        inputData = await provider.fetchIssue(input.issue, options.settings || {});

        // Store issue provider for logging/debugging and cross-provider workflows
        cluster.issueProvider = ProviderClass.id;

        // Log clickable issue link
        if (inputData.url) {
          this._log(`[Orchestrator] Issue (${ProviderClass.displayName}): ${inputData.url}`);
        }
      } else if (input.file) {
        inputData = InputHelpers.createFileInput(input.file);
        this._log(`[Orchestrator] File: ${input.file}`);
      } else if (input.text) {
        inputData = InputHelpers.createTextInput(input.text);
      } else {
        throw new Error('Either issue, file, or text input is required');
      }

      // Detect git platform for --pr mode (independent of issue provider)
      if (options.autoPr) {
        const { detectGitContext } = require('../lib/git-remote-utils');
        const gitContext = detectGitContext(options.cwd);
        cluster.gitPlatform = gitContext?.provider || null;

        if (cluster.gitPlatform) {
          this._log(`[Orchestrator] Git platform detected: ${cluster.gitPlatform.toUpperCase()}`);
        }
      }

      // Inject git-pusher agent if --pr is set (replaces completion-detector)
      this._applyAutoPrConfig(config, inputData, options);

      // Inject workers instruction if --workers explicitly provided and > 1
      this._applyWorkerInstruction(config);

      // Initialize agents with optional mock injection
      // Check agent type: regular agent or subcluster
      // CRITICAL: Inject cwd into each agent config for proper working directory
      // In worktree mode, agents run in the worktree path (not original cwd)
      this._initializeClusterAgents({
        config,
        cluster,
        messageBus,
        options,
        isolationManager,
        clusterId,
      });

      // ========================================================================
      // CRITICAL ORDERING INVARIANT (Issue #31 - Subscription Race Condition)
      // ========================================================================
      // MUST register ALL subscriptions BEFORE starting agents.
      //
      // WHY: EventEmitter is synchronous and doesn't replay past events to new
      // listeners. If an agent completes and publishes CLUSTER_COMPLETE before
      // we register the subscription, the orchestrator will never receive the
      // message and the cluster will appear stuck in 'executing_task' forever.
      //
      // ORDER:
      //   1. Register subscriptions (lines below)
      //   2. Start agents
      //   3. Publish ISSUE_OPENED
      //
      // DO NOT move subscriptions after agent.start() - this will reintroduce
      // the race condition fixed in issue #31.
      // ========================================================================
      this._registerClusterSubscriptions({
        messageBus,
        clusterId,
        isolationManager,
        containerId,
      });

      // Start all agents
      for (const agent of cluster.agents) {
        await agent.start();
      }

      cluster.state = 'running';

      // Publish ISSUE_OPENED message to bootstrap workflow
      messageBus.publish({
        cluster_id: clusterId,
        topic: 'ISSUE_OPENED',
        sender: 'system',
        receiver: 'broadcast',
        content: {
          text: inputData.context,
          data: {
            issue_number: inputData.number,
            title: inputData.title,
          },
        },
        metadata: {
          source: this._getInputSource(input),
        },
      });

      // CRITICAL: Mark initialization complete AFTER ISSUE_OPENED is published
      // This ensures stop() waits for at least 1 message before stopping
      if (cluster._resolveInitComplete) {
        cluster._resolveInitComplete();
      }

      this._log(`Cluster ${clusterId} started with ${cluster.agents.length} agents`);

      // DISABLED: Idle timeout auto-stop mechanism
      // WHY DISABLED: Clusters should only stop on explicit signals:
      //   - User `zeroshot kill` command
      //   - CLUSTER_COMPLETE message (successful completion)
      //   - CLUSTER_FAILED message (failure/abort)
      // Being "idle" is NOT a reason to auto-stop - agents may be legitimately
      // waiting for external events, user input (in interactive mode), or
      // processing that doesn't show as "executing" (e.g., polling, monitoring).
      //
      // Previous behavior: Stopped cluster after 2 minutes of all agents idle
      // Result: Clusters were killed while legitimately waiting, causing confusion
      //
      // cluster.idleCheckInterval = setInterval(() => { ... }, 30000);
      // ^^^^^^ REMOVED - clusters run until explicitly stopped or completed

      // Save cluster to disk
      await this._saveClusters();

      return {
        id: clusterId,
        state: cluster.state,
        agents: cluster.agents.map((a) => a.getState()),
        ledger: cluster.ledger, // Expose ledger for testing
        messageBus: cluster.messageBus, // Expose messageBus for testing
      };
    } catch (error) {
      cluster.state = 'failed';
      // CRITICAL: Resolve the promise on failure too, so stop() doesn't hang
      if (cluster._resolveInitComplete) {
        cluster._resolveInitComplete();
      }
      console.error(`Cluster ${clusterId} failed to start:`, error);
      throw error;
    }
  }

  _initializeClusterAgents({ config, cluster, messageBus, options, isolationManager, clusterId }) {
    const agentCwd = cluster.worktree ? cluster.worktree.path : options.cwd || process.cwd();

    for (const agentConfig of config.agents) {
      if (!agentConfig.cwd) {
        agentConfig.cwd = agentCwd;
      }

      if (options.modelOverride) {
        applyModelOverride(agentConfig, options.modelOverride);
        this._log(`    [model] Overridden model for ${agentConfig.id}: ${options.modelOverride}`);
      }

      const agentOptions = {
        testMode: options.testMode || !!this.taskRunner,
        quiet: this.quiet,
        modelOverride: options.modelOverride || null,
      };

      if (options.mockExecutor) {
        agentOptions.mockSpawnFn = options.mockExecutor.createMockSpawnFn(agentConfig.id);
      }

      if (this.taskRunner) {
        agentOptions.taskRunner = this.taskRunner;
      }

      if (cluster.isolation) {
        agentOptions.isolation = {
          enabled: true,
          manager: isolationManager,
          clusterId,
        };
      }

      if (cluster.worktree) {
        agentOptions.worktree = {
          enabled: true,
          path: cluster.worktree.path,
          branch: cluster.worktree.branch,
          repoRoot: cluster.worktree.repoRoot,
        };
      }

      const agent =
        agentConfig.type === 'subcluster'
          ? new SubClusterWrapper(agentConfig, messageBus, cluster, agentOptions)
          : new AgentWrapper(agentConfig, messageBus, cluster, agentOptions);

      cluster.agents.push(agent);
    }
  }

  _subscribeToClusterTopic(messageBus, clusterId, topic, handler) {
    messageBus.subscribe((message) => {
      if (message.topic === topic && message.cluster_id === clusterId) {
        handler(message);
      }
    });
  }

  _registerClusterCompletionHandlers(messageBus, clusterId) {
    this._subscribeToClusterTopic(messageBus, clusterId, 'CLUSTER_COMPLETE', (message) => {
      this._log(`\n${'='.repeat(80)}`);
      this._log(`✅ CLUSTER COMPLETED SUCCESSFULLY: ${clusterId}`);
      this._log(`${'='.repeat(80)}`);
      this._log(`Reason: ${message.content?.data?.reason || 'unknown'}`);
      this._log(`Initiated by: ${message.sender}`);
      this._log(`${'='.repeat(80)}\n`);

      this.stop(clusterId).catch((err) => {
        console.error(`Failed to auto-stop cluster ${clusterId}:`, err.message);
      });
    });

    this._subscribeToClusterTopic(messageBus, clusterId, 'CLUSTER_FAILED', (message) => {
      this._log(`\n${'='.repeat(80)}`);
      this._log(`❌ CLUSTER FAILED: ${clusterId}`);
      this._log(`${'='.repeat(80)}`);
      this._log(`Reason: ${message.content?.data?.reason || 'unknown'}`);
      this._log(`Agent: ${message.sender}`);
      if (message.content?.text) {
        this._log(`Details: ${message.content.text}`);
      }
      this._log(`${'='.repeat(80)}\n`);

      this.stop(clusterId).catch((err) => {
        console.error(`Failed to auto-stop cluster ${clusterId}:`, err.message);
      });
    });
  }

  _registerAgentErrorHandler(messageBus, clusterId) {
    this._subscribeToClusterTopic(messageBus, clusterId, 'AGENT_ERROR', async (message) => {
      const agentRole = message.content?.data?.role;
      const attempts = message.content?.data?.attempts || 1;

      await this._saveClusters();

      if (agentRole === 'implementation' && attempts >= 3) {
        this._log(`\n${'='.repeat(80)}`);
        this._log(`❌ WORKER AGENT FAILED: ${clusterId}`);
        this._log(`${'='.repeat(80)}`);
        this._log(`Worker agent ${message.sender} failed after ${attempts} attempts`);
        this._log(`Error: ${message.content?.data?.error || 'unknown'}`);
        this._log(`Stopping cluster - worker cannot continue`);
        this._log(`${'='.repeat(80)}\n`);

        this.stop(clusterId).catch((err) => {
          console.error(`Failed to auto-stop cluster ${clusterId}:`, err.message);
        });
      }
    });
  }

  _registerAgentLifecycleHandlers(messageBus, clusterId) {
    messageBus.on('topic:AGENT_LIFECYCLE', async (message) => {
      const event = message.content?.data?.event;
      if (
        [
          'TASK_STARTED',
          'TASK_COMPLETED',
          'PROCESS_SPAWNED',
          'TASK_ID_ASSIGNED',
          'STARTED',
        ].includes(event)
      ) {
        await this._saveClusters();
      }
    });

    messageBus.on('topic:AGENT_LIFECYCLE', (message) => {
      if (message.content?.data?.event !== 'AGENT_STALE_WARNING') return;

      const agentId = message.content?.data?.agent;
      const timeSinceLastOutput = message.content?.data?.timeSinceLastOutput;
      const analysis = message.content?.data?.analysis || 'No analysis available';

      this._log(
        `⚠️  Orchestrator: Agent ${agentId} appears stale (${Math.round(timeSinceLastOutput / 1000)}s no output) but will NOT be killed`
      );
      this._log(`    Analysis: ${analysis}`);
      this._log(
        `    Manual intervention may be needed - use 'zeroshot resume ${clusterId}' if stuck`
      );
    });
  }

  _registerConductorWatchdog(messageBus, clusterId) {
    const timeoutMs = 30000;
    let watchdogTimer = null;
    let completedAt = null;

    this._subscribeToClusterTopic(messageBus, clusterId, 'AGENT_LIFECYCLE', (message) => {
      const event = message.content?.data?.event;
      const role = message.content?.data?.role;

      if (event === 'TASK_COMPLETED' && role === 'conductor') {
        completedAt = Date.now();
        this._log(
          `⏱️  Conductor completed. Watchdog started - expecting CLUSTER_OPERATIONS within ${timeoutMs / 1000}s`
        );

        watchdogTimer = setTimeout(() => {
          const clusterOps = messageBus.query({
            cluster_id: clusterId,
            topic: 'CLUSTER_OPERATIONS',
            limit: 1,
          });
          if (clusterOps.length === 0) {
            console.error(`\n${'='.repeat(80)}`);
            console.error(`🔴 CONDUCTOR WATCHDOG TRIGGERED - CLUSTER_OPERATIONS NEVER RECEIVED`);
            console.error(`${'='.repeat(80)}`);
            console.error(`Conductor completed ${timeoutMs / 1000}s ago but no CLUSTER_OPERATIONS`);
            console.error(`This indicates the conductor's onComplete hook FAILED SILENTLY`);
            console.error(
              `Check: 1) Result parsing 2) Transform script errors 3) Schema validation`
            );
            console.error(`${'='.repeat(80)}\n`);

            messageBus.publish({
              cluster_id: clusterId,
              topic: 'CLUSTER_FAILED',
              sender: 'orchestrator',
              content: {
                text: `Conductor completed but CLUSTER_OPERATIONS never published - hook failure`,
                data: {
                  reason: 'CONDUCTOR_WATCHDOG_TIMEOUT',
                  conductorCompletedAt: completedAt,
                  timeoutMs: timeoutMs,
                },
              },
            });
          }
        }, timeoutMs);
      }
    });

    return {
      clear: () => {
        if (!watchdogTimer) {
          return;
        }
        clearTimeout(watchdogTimer);
        watchdogTimer = null;
        const elapsed = completedAt ? Date.now() - completedAt : 0;
        this._log(
          `✅ CLUSTER_OPERATIONS received (${elapsed}ms after conductor completed) - watchdog cleared`
        );
      },
    };
  }

  _registerClusterOperationsHandler(
    messageBus,
    clusterId,
    isolationManager,
    containerId,
    watchdog
  ) {
    this._subscribeToClusterTopic(messageBus, clusterId, 'CLUSTER_OPERATIONS', (message) => {
      watchdog?.clear?.();

      let operations = message.content?.data?.operations;
      if (typeof operations === 'string') {
        try {
          operations = JSON.parse(operations);
        } catch (e) {
          this._log(`⚠️ CLUSTER_OPERATIONS has invalid operations JSON: ${e.message}`);
          return;
        }
      }

      if (!operations || !Array.isArray(operations)) {
        this._log(`⚠️ CLUSTER_OPERATIONS missing operations array, ignoring`);
        return;
      }

      this._log(`\n${'='.repeat(80)}`);
      this._log(`🔧 CLUSTER_OPERATIONS received from ${message.sender}`);
      this._log(`${'='.repeat(80)}`);
      if (message.content?.data?.reasoning) {
        this._log(`Reasoning: ${message.content.data.reasoning}`);
      }
      this._log(`Operations: ${operations.length}`);
      this._log(`${'='.repeat(80)}\n`);

      this._handleOperations(clusterId, operations, message.sender, {
        isolationManager,
        containerId,
      }).catch((err) => {
        console.error(`Failed to execute CLUSTER_OPERATIONS:`, err.message);
        messageBus.publish({
          cluster_id: clusterId,
          topic: 'CLUSTER_OPERATIONS_FAILED',
          sender: 'orchestrator',
          content: {
            text: `Operation chain failed: ${err.message}`,
            data: {
              error: err.message,
              operations: operations,
            },
          },
        });

        this._log(`\n${'='.repeat(80)}`);
        this._log(`❌ CLUSTER_OPERATIONS FAILED - STOPPING CLUSTER`);
        this._log(`${'='.repeat(80)}`);
        this._log(`Error: ${err.message}`);
        this._log(`${'='.repeat(80)}\n`);

        this.stop(clusterId).catch((stopErr) => {
          console.error(`Failed to stop cluster after operation failure:`, stopErr.message);
        });
      });
    });
  }

  _registerClusterSubscriptions({ messageBus, clusterId, isolationManager, containerId }) {
    this._registerClusterCompletionHandlers(messageBus, clusterId);
    this._registerAgentErrorHandler(messageBus, clusterId);
    this._registerAgentLifecycleHandlers(messageBus, clusterId);

    const watchdog = this._registerConductorWatchdog(messageBus, clusterId);
    this._registerClusterOperationsHandler(
      messageBus,
      clusterId,
      isolationManager,
      containerId,
      watchdog
    );
  }

  async _initializeIsolation(options, config, clusterId) {
    let isolationManager = null;
    let containerId = null;
    let worktreeInfo = null;

    if (options.isolation) {
      if (!IsolationManager.isDockerAvailable()) {
        throw new Error('Docker is not available. Install Docker to use --docker mode.');
      }

      const image = options.isolationImage || 'zeroshot-cluster-base';
      await IsolationManager.ensureImage(image);

      isolationManager = new IsolationManager({ image });
      this._log(`[Orchestrator] Starting cluster in isolation mode (image: ${image})`);

      const workDir = options.cwd || process.cwd();
      const providerName = normalizeProviderName(
        config.forceProvider || config.defaultProvider || loadSettings().defaultProvider || 'claude'
      );
      containerId = await isolationManager.createContainer(clusterId, {
        workDir,
        image,
        noMounts: options.noMounts,
        mounts: options.mounts,
        containerHome: options.containerHome,
        provider: providerName,
      });
      this._log(`[Orchestrator] Container created: ${containerId} (workDir: ${workDir})`);
    } else if (options.worktree) {
      const workDir = options.cwd || process.cwd();

      isolationManager = new IsolationManager({});
      worktreeInfo = isolationManager.createWorktreeIsolation(clusterId, workDir);

      this._log(`[Orchestrator] Starting cluster in worktree isolation mode`);
      this._log(`[Orchestrator] Worktree: ${worktreeInfo.path}`);
      this._log(`[Orchestrator] Branch: ${worktreeInfo.branch}`);
    }

    return { isolationManager, containerId, worktreeInfo };
  }

  _applyAutoPrConfig(config, inputData, options) {
    if (!options.autoPr) {
      return;
    }

    config.agents = config.agents.filter((a) => a.id !== 'completion-detector');

    // Detect git platform (independent of issue provider)
    const { getPlatformForPR } = require('./issue-providers');
    const { detectGitContext } = require('../lib/git-remote-utils');

    let platform;
    try {
      // ALWAYS use git context, regardless of issue provider
      // This allows Jira issues in GitHub repos, etc.
      platform = getPlatformForPR(options.cwd);
    } catch (error) {
      throw new Error(`--pr mode failed: ${error.message}`);
    }

    // Check for repo mismatch (issue URL repo vs current git remote)
    // This catches cases like: running `zeroshot run https://gitlab.com/foo/bar/-/issues/1`
    // from a different repo directory
    let skipCloseIssue = false;
    const gitContext = detectGitContext(options.cwd);
    if (inputData.url && gitContext) {
      const issueHost = this._extractHostFromUrl(inputData.url);
      const gitHost = gitContext.host;

      // Compare hosts - if different platforms, warn and don't close issue
      if (issueHost && gitHost && issueHost !== gitHost) {
        this._log(
          `[Orchestrator] ⚠️  WARNING: Issue is from ${issueHost} but PR will be created on ${gitHost}`
        );
        this._log(
          `[Orchestrator] ⚠️  The PR will NOT auto-close the issue (different repositories)`
        );
        this._log(`[Orchestrator] ⚠️  To fix: run zeroshot from the target repository directory`);
        skipCloseIssue = true;
      }
    }

    // Generate platform-specific git-pusher agent from template
    const { generateGitPusherAgent, isPlatformSupported } = require('./agents/git-pusher-template');

    if (!isPlatformSupported(platform)) {
      throw new Error(
        `Platform '${platform}' does not support --pr mode. Supported: github, gitlab, azure-devops`
      );
    }

    const gitPusherConfig = generateGitPusherAgent(platform);

    // Template replacement for issue context
    const issueRef = skipCloseIssue ? '' : `Closes #${inputData.number || 'unknown'}`;
    gitPusherConfig.prompt = gitPusherConfig.prompt
      .replace(/\{\{issue_number\}\}/g, inputData.number || 'unknown')
      .replace(/\{\{issue_title\}\}/g, inputData.title || 'Implementation')
      .replace(/Closes #\{\{issue_number\}\}/g, issueRef);

    config.agents.push(gitPusherConfig);
    this._log(
      `[Orchestrator] Injected ${platform}-git-pusher agent (issue: ${inputData.number || 'N/A'}, PR platform: ${platform.toUpperCase()})`
    );
  }

  /**
   * Extract hostname from a URL
   * @private
   */
  _extractHostFromUrl(url) {
    try {
      const match = url.match(/https?:\/\/([^/]+)/);
      return match ? match[1] : null;
    } catch {
      return null;
    }
  }

  _applyWorkerInstruction(config) {
    const workersCount = process.env.ZEROSHOT_WORKERS ? parseInt(process.env.ZEROSHOT_WORKERS) : 0;
    if (workersCount <= 1) {
      return;
    }

    const workerAgent = config.agents.find((a) => a.id === 'worker');
    if (!workerAgent) {
      return;
    }

    const instruction = `PARALLELIZATION: Use up to ${workersCount} sub-agents to parallelize your work where appropriate.\n\n`;

    if (!workerAgent.prompt) {
      workerAgent.prompt = instruction;
    } else if (typeof workerAgent.prompt === 'string') {
      workerAgent.prompt = instruction + workerAgent.prompt;
    } else if (workerAgent.prompt.system) {
      workerAgent.prompt.system = instruction + workerAgent.prompt.system;
    }

    this._log(`[Orchestrator] Injected parallelization instruction (workers=${workersCount})`);
  }

  /**
   * Generate a unique cluster ID, safe for concurrent starts in-process.
   * If an explicit ID is provided, uses it as a base and suffixes on collision.
   * @private
   */
  _generateUniqueClusterId(explicitId, explicitDbPath) {
    const baseId = explicitId || generateName('cluster');
    const baseDbPath = explicitDbPath || path.join(this.storageDir, `${baseId}.db`);

    // Fast path: base is unused.
    if (!this.clusters.has(baseId) && !fs.existsSync(baseDbPath)) {
      return baseId;
    }

    // Collision: suffix with random bytes to avoid race conditions under concurrency.
    for (let attempt = 0; attempt < 50; attempt++) {
      const suffix = crypto.randomBytes(3).toString('hex');
      const candidateId = `${baseId}-${suffix}`;
      const candidateDbPath = explicitDbPath || path.join(this.storageDir, `${candidateId}.db`);
      if (!this.clusters.has(candidateId) && !fs.existsSync(candidateDbPath)) {
        return candidateId;
      }
    }

    // Last resort: new generated name (should never happen).
    for (let attempt = 0; attempt < 50; attempt++) {
      const candidateId = generateName('cluster');
      const candidateDbPath = explicitDbPath || path.join(this.storageDir, `${candidateId}.db`);
      if (!this.clusters.has(candidateId) && !fs.existsSync(candidateDbPath)) {
        return candidateId;
      }
    }

    throw new Error('Failed to generate unique cluster ID after many attempts');
  }

  /**
   * Stop a cluster
   * @param {String} clusterId - Cluster ID
   */
  async stop(clusterId) {
    const cluster = this.clusters.get(clusterId);
    if (!cluster) {
      throw new Error(`Cluster ${clusterId} not found`);
    }

    // CRITICAL: Wait for initialization to complete before stopping
    // This ensures ISSUE_OPENED is published, preventing 0-message clusters
    // Timeout after 30s to prevent infinite hang if init truly fails
    if (cluster.initCompletePromise && cluster.state === 'initializing') {
      this._log(`[Orchestrator] Waiting for initialization to complete before stopping...`);
      await Promise.race([
        cluster.initCompletePromise,
        new Promise((resolve) => setTimeout(resolve, 30000)),
      ]);
    }

    cluster.state = 'stopping';

    // Stop all agents (including subclusters which handle their own children)
    for (const agent of cluster.agents) {
      await agent.stop();
    }

    if (cluster.snapshotter) {
      cluster.snapshotter.stop();
    }

    // Clean up isolation container if enabled
    // CRITICAL: Preserve workspace for resume capability - only delete on kill()
    if (cluster.isolation?.manager) {
      this._log(
        `[Orchestrator] Stopping isolation container for ${clusterId} (preserving workspace for resume)...`
      );
      await cluster.isolation.manager.cleanup(clusterId, { preserveWorkspace: true });
      this._log(`[Orchestrator] Container stopped, workspace preserved`);
    }

    if (cluster.validatorIsolation?.manager) {
      this._log(`[Orchestrator] Cleaning up validator isolation container for ${clusterId}...`);
      await cluster.validatorIsolation.manager.cleanup(cluster.validatorIsolation.clusterId, {
        preserveWorkspace: false,
      });
      cluster.validatorIsolation = null;
    }

    // Worktree cleanup on stop: preserve for resume capability
    // Branch stays, worktree stays - can resume work later
    if (cluster.worktree?.manager) {
      this._log(`[Orchestrator] Worktree preserved at ${cluster.worktree.path} for resume`);
      this._log(`[Orchestrator] Branch: ${cluster.worktree.branch}`);
      // Don't cleanup worktree - it will be reused on resume
    }

    cluster.state = 'stopped';
    cluster.pid = null; // Clear PID - cluster is no longer running
    this._log(`Cluster ${clusterId} stopped`);

    // Save updated state
    await this._saveClusters();
  }

  /**
   * Kill a cluster (force stop)
   * @param {String} clusterId - Cluster ID
   */
  async kill(clusterId) {
    const cluster = this.clusters.get(clusterId);
    if (!cluster) {
      throw new Error(`Cluster ${clusterId} not found`);
    }

    cluster.state = 'stopping';

    // Force stop all agents
    for (const agent of cluster.agents) {
      await agent.stop();
    }

    if (cluster.snapshotter) {
      cluster.snapshotter.stop();
    }

    // Force remove isolation container AND workspace (full cleanup, no resume)
    if (cluster.isolation?.manager) {
      this._log(
        `[Orchestrator] Force removing isolation container and workspace for ${clusterId}...`
      );
      await cluster.isolation.manager.cleanup(clusterId, { preserveWorkspace: false });
      this._log(`[Orchestrator] Container and workspace removed`);
    }

    if (cluster.validatorIsolation?.manager) {
      this._log(`[Orchestrator] Force removing validator isolation container for ${clusterId}...`);
      await cluster.validatorIsolation.manager.cleanup(cluster.validatorIsolation.clusterId, {
        preserveWorkspace: false,
      });
      cluster.validatorIsolation = null;
    }

    // Force remove worktree (full cleanup, no resume)
    // Note: Branch is preserved for potential PR creation / inspection
    if (cluster.worktree?.manager) {
      this._log(`[Orchestrator] Force removing worktree for ${clusterId}...`);
      cluster.worktree.manager.cleanupWorktreeIsolation(clusterId, { preserveBranch: true });
      this._log(`[Orchestrator] Worktree removed, branch ${cluster.worktree.branch} preserved`);
    }

    // Close message bus and ledger
    cluster.messageBus.close();

    cluster.state = 'killed';
    cluster.pid = null; // Clear PID - cluster is no longer running
    // DON'T delete from memory - keep it so it gets saved with 'killed' state
    // this.clusters.delete(clusterId);

    this._log(`Cluster ${clusterId} killed`);

    // Save updated state (will be marked as 'killed' in file)
    await this._saveClusters();

    // Now remove from memory after persisting
    this.clusters.delete(clusterId);
  }

  /**
   * Kill all running clusters
   * @returns {Object} { killed: Array<string>, errors: Array<{id, error}> }
   */
  async killAll() {
    const results = { killed: [], errors: [] };
    const runningClusters = Array.from(this.clusters.values()).filter(
      (c) => c.state === 'running' || c.state === 'initializing'
    );

    for (const cluster of runningClusters) {
      try {
        await this.kill(cluster.id);
        results.killed.push(cluster.id);
      } catch (error) {
        results.errors.push({ id: cluster.id, error: error.message });
      }
    }

    return results;
  }

  /**
   * Close the orchestrator (prevents further _saveClusters operations)
   * Call before deleting storageDir to prevent ENOENT race conditions during cleanup
   */
  close() {
    this.closed = true;
  }

  /**
   * Find the last workflow-triggering message in the ledger
   * Workflow triggers indicate cluster state progression (not AGENT_OUTPUT noise)
   * @param {Array} messages - All messages from ledger
   * @returns {Object|null} - Last workflow trigger message or null
   * @private
   */
  _findLastWorkflowTrigger(messages) {
    for (let i = messages.length - 1; i >= 0; i--) {
      if (WORKFLOW_TRIGGERS.includes(messages[i].topic)) {
        return messages[i];
      }
    }
    return null;
  }

  /**
   * Resume a stopped cluster from where it left off
   * Handles both failed clusters (with error context) and cleanly stopped clusters
   * @param {String} clusterId - Cluster ID
   * @param {String} prompt - Optional custom resume prompt
   * @returns {Object} Resumed cluster info
   */
  async resume(clusterId, prompt) {
    const cluster = this.clusters.get(clusterId);
    if (!cluster) {
      throw new Error(`Cluster not found: ${clusterId}`);
    }

    if (cluster.state === 'running') {
      throw new Error(
        `Cluster ${clusterId} is still running. Use 'zeroshot stop' first if you want to restart it.`
      );
    }

    const failureInfo = this._resolveFailureInfo(cluster, clusterId);

    await this._ensureIsolationForResume(clusterId, cluster);
    this._ensureWorktreeForResume(clusterId, cluster);
    this._startSnapshotter(cluster);
    await this._restartClusterAgents(cluster);

    const recentMessages = this._loadRecentMessages(cluster, clusterId, 50);

    if (failureInfo) {
      return this._resumeFailedCluster(clusterId, cluster, failureInfo, recentMessages, prompt);
    }

    return this._resumeCleanCluster(clusterId, cluster, recentMessages, prompt);
  }

  _resolveFailureInfo(cluster, clusterId) {
    if (cluster.failureInfo) {
      return cluster.failureInfo;
    }

    const errors = cluster.messageBus.query({
      cluster_id: clusterId,
      topic: 'AGENT_ERROR',
      limit: 10,
      order: 'desc',
    });

    if (errors.length === 0) {
      return null;
    }

    const firstError = errors[0];
    const failureInfo = {
      agentId: firstError.sender,
      taskId: firstError.content?.data?.taskId || null,
      iteration: firstError.content?.data?.iteration || 0,
      error: firstError.content?.data?.error || firstError.content?.text,
      timestamp: firstError.timestamp,
    };
    this._log(`[Orchestrator] Found failure from ledger: ${failureInfo.agentId}`);
    return failureInfo;
  }

  _checkContainerExists(containerId) {
    const { spawn } = require('child_process');
    const checkContainer = spawn('docker', ['inspect', containerId], { stdio: 'ignore' });
    return new Promise((resolve) => {
      checkContainer.on('close', (code) => resolve(code === 0));
    });
  }

  async _ensureIsolationForResume(clusterId, cluster) {
    if (!cluster.isolation?.enabled) {
      return;
    }

    const oldContainerId = cluster.isolation.containerId;
    const containerExists = await this._checkContainerExists(oldContainerId);
    if (containerExists) {
      this._log(`[Orchestrator] Container ${oldContainerId} still exists, reusing`);
      return;
    }

    this._log(`[Orchestrator] Container ${oldContainerId} not found, recreating...`);

    const workDir = cluster.isolation.workDir;
    if (!workDir) {
      throw new Error(`Cannot resume cluster ${clusterId}: workDir not saved in isolation state`);
    }

    const isolatedPath = path.join(os.tmpdir(), 'zeroshot-isolated', clusterId);
    if (!fs.existsSync(isolatedPath)) {
      throw new Error(
        `Cannot resume cluster ${clusterId}: isolated workspace deleted. ` +
          `Was the cluster killed (not stopped)? Use 'zeroshot run' to start fresh.`
      );
    }

    const providerName = normalizeProviderName(
      cluster.config?.forceProvider ||
        cluster.config?.defaultProvider ||
        loadSettings().defaultProvider ||
        'claude'
    );
    const newContainerId = await cluster.isolation.manager.createContainer(clusterId, {
      workDir,
      image: cluster.isolation.image,
      reuseExistingWorkspace: true,
      provider: providerName,
    });

    this._log(`[Orchestrator] New container created: ${newContainerId}`);
    cluster.isolation.containerId = newContainerId;

    for (const agent of cluster.agents) {
      if (agent.isolation?.enabled) {
        agent.isolation.containerId = newContainerId;
        agent.isolation.manager = cluster.isolation.manager;
      }
    }

    this._log(`[Orchestrator] All agents updated with new container ID`);
  }

  _ensureWorktreeForResume(clusterId, cluster) {
    if (!cluster.worktree?.enabled) {
      return;
    }

    const worktreePath = cluster.worktree.path;
    if (!fs.existsSync(worktreePath)) {
      throw new Error(
        `Cannot resume cluster ${clusterId}: worktree at ${worktreePath} no longer exists. ` +
          `Was the worktree manually removed? Use 'zeroshot run' to start fresh.`
      );
    }

    this._log(`[Orchestrator] Worktree at ${worktreePath} exists, reusing`);
    this._log(`[Orchestrator] Branch: ${cluster.worktree.branch}`);
  }

  async _restartClusterAgents(cluster) {
    cluster.state = 'running';
    for (const agent of cluster.agents) {
      if (!agent.running) {
        await agent.start();
      }
    }
  }

  _loadRecentMessages(cluster, clusterId, limit) {
    return cluster.messageBus
      .query({
        cluster_id: clusterId,
        limit,
        order: 'desc',
      })
      .reverse();
  }

  _buildResumeContext(recentMessages, prompt, options) {
    const resumePrompt = prompt || 'Continue from where you left off. Complete the task.';
    let context = `${options.header}\n\n`;

    if (options.error) {
      context += `Previous error: ${options.error}\n\n`;
    }

    context += `## Recent Context\n\n`;
    for (const msg of recentMessages.slice(-10)) {
      if (options.topics.includes(msg.topic)) {
        context += `[${msg.sender}] ${msg.content?.text?.slice(0, 200) || ''}\n`;
      }
    }

    context += `\n## Resume Instructions\n\n${resumePrompt}\n`;
    return context;
  }

  _triggerMatchesTopic(triggerTopic, topic) {
    if (triggerTopic === topic) return true;
    if (triggerTopic === '*') return true;
    if (triggerTopic.endsWith('*')) {
      const prefix = triggerTopic.slice(0, -1);
      return topic.startsWith(prefix);
    }
    return false;
  }

  _selectAgentsToResume(cluster, lastTrigger) {
    const agentsToResume = [];

    for (const agent of cluster.agents) {
      if (!agent.config.triggers) continue;

      const matchingTrigger = agent.config.triggers.find((trigger) =>
        this._triggerMatchesTopic(trigger.topic, lastTrigger.topic)
      );
      if (!matchingTrigger) continue;

      if (matchingTrigger.logic?.script) {
        const shouldTrigger = agent._evaluateTrigger(matchingTrigger, lastTrigger);
        if (!shouldTrigger) continue;
      }

      agentsToResume.push({ agent, message: lastTrigger, trigger: matchingTrigger });
    }

    return agentsToResume;
  }

  _resumeAgents(agentsToResume, context) {
    for (const { agent, message } of agentsToResume) {
      this._log(`[Orchestrator] - Resuming agent ${agent.id} (triggered by ${message.topic})`);
      agent.resume(context).catch((err) => {
        console.error(`[Orchestrator] Resume failed for agent ${agent.id}:`, err.message);
      });
    }
  }

  _republishIssue(cluster, clusterId, recentMessages) {
    const issueMessage = recentMessages.find((m) => m.topic === 'ISSUE_OPENED');
    if (!issueMessage) {
      return false;
    }

    cluster.messageBus.publish({
      cluster_id: clusterId,
      topic: 'ISSUE_OPENED',
      sender: 'system',
      receiver: 'broadcast',
      content: issueMessage.content,
      metadata: { _resumed: true, _originalId: issueMessage.id },
    });

    return true;
  }

  async _resumeFailedCluster(clusterId, cluster, failureInfo, recentMessages, prompt) {
    const { agentId, iteration, error } = failureInfo;
    this._log(
      `[Orchestrator] Resuming failed cluster ${clusterId} from agent ${agentId} iteration ${iteration}`
    );
    this._log(`[Orchestrator] Previous error: ${error}`);

    const failedAgent = cluster.agents.find((agent) => agent.id === agentId);
    if (!failedAgent) {
      throw new Error(`Failed agent '${agentId}' not found in cluster`);
    }

    const context = this._buildResumeContext(recentMessages, prompt, {
      header: 'You are resuming from a previous failed attempt.',
      error,
      topics: ['AGENT_OUTPUT', 'VALIDATION_RESULT'],
    });

    cluster.failureInfo = null;
    await this._saveClusters();

    failedAgent.resume(context).catch((err) => {
      console.error(`[Orchestrator] Resume failed for agent ${agentId}:`, err.message);
    });

    this._log(`[Orchestrator] Cluster ${clusterId} resumed from failure`);

    return {
      id: clusterId,
      state: cluster.state,
      resumeType: 'failure',
      resumedAgent: agentId,
      previousError: error,
    };
  }

  async _resumeCleanCluster(clusterId, cluster, recentMessages, prompt) {
    this._log(`[Orchestrator] Resuming stopped cluster ${clusterId} (no failure)`);

    const context = this._buildResumeContext(recentMessages, prompt, {
      header: 'Resuming cluster from previous session.',
      topics: ['AGENT_OUTPUT', 'VALIDATION_RESULT', 'ISSUE_OPENED'],
    });

    const lastTrigger = this._findLastWorkflowTrigger(recentMessages);
    if (lastTrigger) {
      this._log(
        `[Orchestrator] Last workflow trigger: ${lastTrigger.topic} (${new Date(lastTrigger.timestamp).toISOString()})`
      );
    } else {
      this._log(`[Orchestrator] No workflow triggers found in ledger`);
    }

    const agentsToResume = lastTrigger ? this._selectAgentsToResume(cluster, lastTrigger) : [];
    if (agentsToResume.length === 0) {
      if (!lastTrigger) {
        this._log(
          `[Orchestrator] WARNING: No workflow triggers in ledger. Cluster may not have started properly.`
        );
        this._log(`[Orchestrator] Publishing ISSUE_OPENED to bootstrap workflow...`);

        if (!this._republishIssue(cluster, clusterId, recentMessages)) {
          throw new Error(
            `Cannot resume cluster ${clusterId}: No workflow triggers found and no ISSUE_OPENED message. ` +
              `The cluster may not have started properly. Try: zeroshot run <issue> instead.`
          );
        }
      } else {
        throw new Error(
          `Cannot resume cluster ${clusterId}: Found trigger ${lastTrigger.topic} but no agents handle it. ` +
            `Check agent trigger configurations.`
        );
      }
    } else {
      this._log(`[Orchestrator] Resuming ${agentsToResume.length} agent(s) based on ledger state`);
      this._resumeAgents(agentsToResume, context);
    }

    await this._saveClusters();
    this._log(`[Orchestrator] Cluster ${clusterId} resumed`);

    return {
      id: clusterId,
      state: cluster.state,
      resumeType: 'clean',
      resumedAgents: agentsToResume.map((a) => a.agent.id),
    };
  }

  /**
   * Force restart a stale agent with imperative prompt injection
   * @param {string} clusterId - Cluster ID
   * @param {string} agentId - Agent to restart
   * @param {number} staleDuration - How long agent was stale (ms)
   * @private
   */
  async _forceRestartAgent(clusterId, agentId, staleDuration) {
    const cluster = this.clusters.get(clusterId);
    if (!cluster) {
      throw new Error(`Cluster ${clusterId} not found`);
    }

    const agent = cluster.agents.find((a) => a.id === agentId);
    if (!agent) {
      throw new Error(`Agent ${agentId} not found in cluster ${clusterId}`);
    }

    // Kill current task
    try {
      agent._killTask();
    } catch (err) {
      this._log(`⚠️  Failed to kill agent ${agentId} task:`, err.message);
    }

    // Build imperative restart context
    const staleMinutes = Math.round(staleDuration / 60000);
    const imperativePrompt = `
🔴 CRITICAL: Your previous session STOPPED PRODUCING OUTPUT for ${staleMinutes} minutes and was detected as STUCK.

## What Happened
- Last output timestamp: ${new Date(Date.now() - staleDuration).toISOString()}
- Detected as stale after ${staleMinutes} minutes of silence
- Process was forcefully restarted

## Your Instructions
You MUST complete your current task. DO NOT STOP until you either:
1. Successfully complete the task and publish your completion message, OR
2. Explicitly state WHY you cannot complete the task (missing files, impossible requirements, etc.)

If you discovered that files you need to modify don't exist:
- CREATE them from scratch with the expected implementation
- DO NOT silently give up
- DO NOT stop working without explicit explanation

If you are stuck in an impossible situation:
- EXPLAIN the problem clearly
- PROPOSE alternative solutions
- WAIT for guidance - do not exit

## Resume Your Work
Continue from where you left off. Review your previous output to understand what you were working on.
`.trim();

    // Get recent context from ledger
    const recentMessages = cluster.messageBus
      .query({
        cluster_id: cluster.id,
        limit: 10,
        order: 'desc',
      })
      .reverse();

    const contextText = recentMessages
      .map((m) => `[${m.sender}] ${m.content?.text || JSON.stringify(m.content)}`)
      .join('\n\n');

    const fullContext = `${imperativePrompt}\n\n## Recent Context\n${contextText}`;

    // Resume agent with imperative prompt
    this._log(
      `🔄 Restarting agent ${agentId} with imperative prompt (${imperativePrompt.length} chars)`
    );

    try {
      await agent.resume(fullContext);
      this._log(`✅ Agent ${agentId} successfully restarted`);
    } catch (err) {
      this._log(`❌ Failed to resume agent ${agentId}:`, err.message);
      throw err;
    }
  }

  /**
   * Handle operation chain from CLUSTER_OPERATIONS message
   * Executes operations sequentially: add_agents, remove_agents, update_agent, publish
   *
   * Validation strategy:
   * 1. Pre-validate all agent configs before executing any operations
   * 2. Build a mock cluster config with proposed changes
   * 3. Run config-validator on the mock to catch structural issues
   * 4. Only execute operations if validation passes
   *
   * @param {string} clusterId - Cluster ID
   * @param {Array} operations - Array of operation objects
   * @param {string} sender - Who sent the operations (for attribution)
   * @param {Object} context - Isolation context { isolationManager, containerId }
   * @private
   */
  async _handleOperations(clusterId, operations, sender, context = {}) {
    const cluster = this.clusters.get(clusterId);
    if (!cluster) {
      throw new Error(`Cluster ${clusterId} not found`);
    }

    this._log(`[Orchestrator] Validating ${operations.length} operation(s) from ${sender}`);

    // Phase 1: Pre-validate operation structure
    const validationErrors = this._validateOperationChain(operations);

    if (validationErrors.length > 0) {
      const errorMsg = `Operation chain validation failed:\n  - ${validationErrors.join('\n  - ')}`;
      this._log(`[Orchestrator] ❌ ${errorMsg}`);
      throw new Error(errorMsg);
    }

    // Phase 2: Build mock cluster config with proposed agents
    // Collect all agents that would exist after operations complete
    const existingAgentConfigs = cluster.config.agents || [];
    const proposedAgentConfigs = this._buildProposedAgentConfigs(existingAgentConfigs, operations);

    // Phase 3: Validate proposed cluster config
    const validation = this._validateProposedConfig(
      clusterId,
      cluster,
      proposedAgentConfigs,
      operations
    );

    // Log warnings but proceed
    if (validation.warnings.length > 0) {
      this._log(`[Orchestrator] ⚠️ Warnings (proceeding anyway):`);
      for (const warning of validation.warnings) {
        this._log(`    - ${warning}`);
      }
    }

    // Phase 4: Execute validated operations
    this._log(`[Orchestrator] ✓ Validation passed, executing ${operations.length} operation(s)`);

    await this._executeOperations(cluster, operations, sender, context);

    this._log(`[Orchestrator] All ${operations.length} operation(s) executed successfully`);

    // Publish success notification
    cluster.messageBus.publish({
      cluster_id: clusterId,
      topic: 'CLUSTER_OPERATIONS_SUCCESS',
      sender: 'orchestrator',
      content: {
        text: `Executed ${operations.length} operation(s)`,
        data: {
          operationCount: operations.length,
          agentCount: cluster.agents.length,
        },
      },
    });

    // Save updated cluster state to disk
    await this._saveClusters();
  }

  _validateOperationChain(operations) {
    const validationErrors = [];

    for (let i = 0; i < operations.length; i++) {
      const op = operations[i];
      if (!op.action) {
        validationErrors.push(`Operation ${i}: missing 'action' field`);
        continue;
      }
      if (!VALID_OPERATIONS.includes(op.action)) {
        validationErrors.push(
          `Operation ${i}: unknown action '${op.action}'. Valid: ${VALID_OPERATIONS.join(', ')}`
        );
      }
    }

    return validationErrors;
  }

  _buildProposedAgentConfigs(existingAgentConfigs, operations) {
    const proposedAgentConfigs = [...existingAgentConfigs];

    for (const op of operations) {
      if (op.action === 'add_agents' && op.agents) {
        for (const agentConfig of op.agents) {
          const existingIdx = proposedAgentConfigs.findIndex((a) => a.id === agentConfig.id);
          if (existingIdx === -1) {
            proposedAgentConfigs.push(agentConfig);
          }
        }
      } else if (op.action === 'remove_agents' && op.agentIds) {
        for (const agentId of op.agentIds) {
          const idx = proposedAgentConfigs.findIndex((a) => a.id === agentId);
          if (idx !== -1) {
            proposedAgentConfigs.splice(idx, 1);
          }
        }
      } else if (op.action === 'update_agent' && op.agentId && op.updates) {
        const agentConfig = proposedAgentConfigs.find((a) => a.id === op.agentId);
        if (agentConfig) {
          Object.assign(agentConfig, op.updates);
        }
      }
    }

    return proposedAgentConfigs;
  }

  _validateProposedConfig(clusterId, cluster, proposedAgentConfigs, operations) {
    const mockConfig = { agents: proposedAgentConfigs };
    const validation = configValidator.validateConfig(mockConfig);

    if (!validation.valid) {
      const errorMsg = `Proposed cluster configuration is invalid:\n  - ${validation.errors.join('\n  - ')}`;
      this._log(`[Orchestrator] ❌ ${errorMsg}`);

      // Publish validation failure for conductor to see and retry
      cluster.messageBus.publish({
        cluster_id: clusterId,
        topic: 'CLUSTER_OPERATIONS_VALIDATION_FAILED',
        sender: 'orchestrator',
        content: {
          text: 'Operation chain would create invalid cluster configuration',
          data: {
            errors: validation.errors,
            warnings: validation.warnings,
            operations: operations,
          },
        },
      });

      throw new Error(errorMsg);
    }

    return validation;
  }

  async _executeOperations(cluster, operations, sender, context) {
    for (let i = 0; i < operations.length; i++) {
      const op = operations[i];
      this._log(`  [${i + 1}/${operations.length}] ${op.action}`);
      await this._executeOperation(cluster, op, sender, context);
    }
  }

  async _executeOperation(cluster, op, sender, context) {
    switch (op.action) {
      case 'add_agents':
        await this._opAddAgents(cluster, op, context);
        break;
      case 'remove_agents':
        await this._opRemoveAgents(cluster, op);
        break;
      case 'update_agent':
        this._opUpdateAgent(cluster, op);
        break;
      case 'publish':
        this._opPublish(cluster, op, sender);
        break;
      case 'load_config':
        await this._opLoadConfig(cluster, op, context);
        break;
    }
  }

  /**
   * Operation: add_agents - Spawn new agents dynamically
   * @private
   */
  async _opAddAgents(cluster, op, context) {
    const agents = op.agents;
    if (!agents || !Array.isArray(agents)) {
      throw new Error('add_agents operation missing agents array');
    }

    // CRITICAL: Inject cluster's worktree cwd into dynamically added agents
    // Without this, template agents (planning, implementation, validator) run in
    // original directory instead of worktree, causing --ship mode to pollute main
    const agentCwd = cluster.worktree?.path || cluster.isolation?.workDir || process.cwd();

    for (const agentConfig of agents) {
      // Inject cwd if not already set (same as startCluster does for initial agents)
      if (!agentConfig.cwd) {
        agentConfig.cwd = agentCwd;
        this._log(`    [cwd] Injected worktree cwd for ${agentConfig.id}: ${agentCwd}`);
      }

      // Apply model override if set (for consistency with initial agents)
      if (cluster.modelOverride) {
        applyModelOverride(agentConfig, cluster.modelOverride);
        this._log(`    [model] Overridden model for ${agentConfig.id}: ${cluster.modelOverride}`);
      }
      // Validate agent config has required fields
      if (!agentConfig.id) {
        throw new Error('Agent config missing required field: id');
      }

      // Check for duplicate agent ID
      const existingAgent = cluster.agents.find((a) => a.id === agentConfig.id);
      if (existingAgent) {
        this._log(`    ⚠️ Agent ${agentConfig.id} already exists, skipping`);
        continue;
      }

      // Add to config agents array (for persistence)
      if (!cluster.config.agents) {
        cluster.config.agents = [];
      }
      cluster.config.agents.push(agentConfig);

      // Build agent options
      const agentOptions = {
        testMode: !!this.taskRunner, // Enable testMode if taskRunner provided
        quiet: this.quiet,
        modelOverride: cluster.modelOverride || null,
      };

      // TaskRunner DI - propagate to dynamically spawned agents
      if (this.taskRunner) {
        agentOptions.taskRunner = this.taskRunner;
      }

      // Pass isolation context if cluster is running in isolation mode
      if (cluster.isolation?.enabled && context.isolationManager) {
        agentOptions.isolation = {
          enabled: true,
          manager: context.isolationManager,
          clusterId: cluster.id,
        };
      }

      // Create and start agent
      const agent = new AgentWrapper(agentConfig, cluster.messageBus, cluster, agentOptions);
      cluster.agents.push(agent);
      await agent.start();

      this._log(
        `    ✓ Added agent: ${agentConfig.id} (role: ${agentConfig.role || 'unspecified'})`
      );
    }
  }

  /**
   * Operation: remove_agents - Stop and remove agents by ID
   * @private
   */
  async _opRemoveAgents(cluster, op) {
    const agentIds = op.agentIds;
    if (!agentIds || !Array.isArray(agentIds)) {
      throw new Error('remove_agents operation missing agentIds array');
    }

    for (const agentId of agentIds) {
      const agentIndex = cluster.agents.findIndex((a) => a.id === agentId);
      if (agentIndex === -1) {
        this._log(`    ⚠️ Agent ${agentId} not found, skipping removal`);
        continue;
      }

      const agent = cluster.agents[agentIndex];
      await agent.stop();

      // Remove from cluster.agents
      cluster.agents.splice(agentIndex, 1);

      // Remove from config.agents
      if (cluster.config.agents) {
        const configIndex = cluster.config.agents.findIndex((a) => a.id === agentId);
        if (configIndex !== -1) {
          cluster.config.agents.splice(configIndex, 1);
        }
      }

      this._log(`    ✓ Removed agent: ${agentId}`);
    }
  }

  /**
   * Operation: update_agent - Modify existing agent config at runtime
   * Note: Some updates may require agent restart to take effect
   * @private
   */
  _opUpdateAgent(cluster, op) {
    const { agentId, updates } = op;
    if (!agentId) {
      throw new Error('update_agent operation missing agentId');
    }
    if (!updates || typeof updates !== 'object') {
      throw new Error('update_agent operation missing updates object');
    }

    const agent = cluster.agents.find((a) => a.id === agentId);
    if (!agent) {
      throw new Error(`update_agent: Agent ${agentId} not found`);
    }

    // Apply updates to agent config
    Object.assign(agent.config, updates);

    // Also update in cluster.config.agents for persistence
    if (cluster.config.agents) {
      const configAgent = cluster.config.agents.find((a) => a.id === agentId);
      if (configAgent) {
        Object.assign(configAgent, updates);
      }
    }

    this._log(`    ✓ Updated agent: ${agentId} (fields: ${Object.keys(updates).join(', ')})`);
  }

  /**
   * Operation: publish - Publish a message to the bus
   * @private
   */
  _opPublish(cluster, op, sender) {
    const { topic, content, metadata } = op;
    if (!topic) {
      throw new Error('publish operation missing topic');
    }

    cluster.messageBus.publish({
      cluster_id: cluster.id,
      topic,
      sender: op.sender || sender,
      receiver: op.receiver || 'broadcast',
      content: content || {},
      metadata: metadata || null,
    });

    this._log(`    ✓ Published to topic: ${topic}`);
  }

  /**
   * Operation: load_config - Load agents from a cluster config
   *
   * Supports two formats:
   * 1. Static config: { config: 'config-name' } - loads from cluster-templates/config-name.json
   * 2. Parameterized: { config: { base: 'template-name', params: {...} } } - resolves base template with params
   *
   * @private
   */
  async _opLoadConfig(cluster, op, context) {
    const { config } = op;
    if (!config) {
      throw new Error('load_config operation missing config');
    }

    const templatesDir = path.join(__dirname, '..', 'cluster-templates');
    let loadedConfig;

    // Check if config is parameterized ({ base, params }) or static (string)
    if (typeof config === 'object' && config.base) {
      // Parameterized template - resolve with TemplateResolver
      const { base, params } = config;
      this._log(`    Loading parameterized template: ${base}`);
      this._log(`    Params: ${JSON.stringify(params)}`);

      const resolver = new TemplateResolver(templatesDir);
      loadedConfig = resolver.resolve(base, params);

      this._log(`    ✓ Resolved template: ${base} → ${loadedConfig.agents?.length || 0} agent(s)`);
    } else if (typeof config === 'string') {
      // Static config - load directly from file
      const configPath = path.join(templatesDir, `${config}.json`);

      if (!fs.existsSync(configPath)) {
        throw new Error(`Config not found: ${config} (looked in ${configPath})`);
      }

      this._log(`    Loading static config: ${config}`);

      const configContent = fs.readFileSync(configPath, 'utf8');
      loadedConfig = JSON.parse(configContent);
    } else {
      throw new Error(
        `Invalid config format: expected string or {base, params}, got ${typeof config}`
      );
    }

    if (!loadedConfig.agents || !Array.isArray(loadedConfig.agents)) {
      throw new Error(`Config has no agents array`);
    }

    this._log(`    Found ${loadedConfig.agents.length} agent(s)`);

    // Add agents from loaded config (reuse existing add_agents logic)
    await this._opAddAgents(cluster, { agents: loadedConfig.agents }, context);

    this._log(`    ✓ Config loaded (${loadedConfig.agents.length} agents)`);

    // Inject completion agent (templates don't include one - orchestrator controls termination)
    await this._injectCompletionAgent(cluster, context);
  }

  /**
   * Inject appropriate completion agent based on mode
   * Templates define work, orchestrator controls termination strategy
   * @private
   */
  async _injectCompletionAgent(cluster, context) {
    // Skip if completion agent already exists
    const hasCompletionAgent = cluster.agents.some(
      (a) => a.config?.id === 'completion-detector' || a.config?.id === 'git-pusher'
    );
    if (hasCompletionAgent) {
      return;
    }

    const isPrMode = cluster.autoPr || process.env.ZEROSHOT_PR === '1';

    if (isPrMode) {
      // Detect platform from stored cluster metadata OR git context
      let platform = cluster.gitPlatform; // Use stored git platform (not issueProvider!)

      // Fallback to git context detection if cluster metadata missing
      if (!platform) {
        const { getPlatformForPR } = require('./issue-providers');
        try {
          platform = getPlatformForPR(cluster.cwd || process.cwd());
        } catch (error) {
          throw new Error(`Cannot determine platform for PR creation on resume: ${error.message}`);
        }
      }

      // Generate platform-specific git-pusher agent from template
      const {
        generateGitPusherAgent,
        isPlatformSupported,
      } = require('./agents/git-pusher-template');

      if (!isPlatformSupported(platform)) {
        throw new Error(
          `Platform '${platform}' does not support --pr mode. Supported: github, gitlab, azure-devops`
        );
      }

      const gitPusherConfig = generateGitPusherAgent(platform);

      // Get issue context from ledger
      const issueMsg = cluster.messageBus.ledger.findLast({ topic: 'ISSUE_OPENED' });
      const issueNumber = issueMsg?.content?.data?.number || 'unknown';
      const issueTitle = issueMsg?.content?.data?.title || 'Implementation';

      // Inject issue context into prompt
      gitPusherConfig.prompt = gitPusherConfig.prompt
        .replace(/\{\{issue_number\}\}/g, issueNumber)
        .replace(/\{\{issue_title\}\}/g, issueTitle);

      await this._opAddAgents(cluster, { agents: [gitPusherConfig] }, context);
      this._log(`    [--pr mode] Injected ${platform}-git-pusher agent`);
    } else {
      // Default completion-detector
      const completionDetector = {
        id: 'completion-detector',
        role: 'orchestrator',
        model: 'haiku',
        timeout: 0,
        triggers: [
          {
            topic: 'VALIDATION_RESULT',
            logic: {
              engine: 'javascript',
              script: `const validators = cluster.getAgentsByRole('validator');
const lastPush = ledger.findLast({ topic: 'IMPLEMENTATION_READY' });
if (!lastPush) return false;
if (validators.length === 0) return true;

const validatorIds = new Set(validators.map((v) => v.id));
const results = ledger.query({ topic: 'VALIDATION_RESULT', since: lastPush.timestamp });

const latestByValidator = new Map();
for (const msg of results) {
  if (!validatorIds.has(msg.sender)) continue;
  latestByValidator.set(msg.sender, msg);
}

if (latestByValidator.size < validators.length) return false;

for (const validator of validators) {
  const msg = latestByValidator.get(validator.id);
  const approved = msg?.content?.data?.approved;
  if (!(approved === true || approved === 'true')) return false;
}

return true;`,
            },
            action: 'stop_cluster',
          },
        ],
      };

      await this._opAddAgents(cluster, { agents: [completionDetector] }, context);
      this._log(`    Injected completion-detector agent`);
    }
  }

  /**
   * Check if a process with given PID is running
   * @param {Number} pid - Process ID
   * @returns {Boolean} True if process exists
   * @private
   */
  _isProcessRunning(pid) {
    if (!pid) return false;
    try {
      // Signal 0 doesn't kill, just checks if process exists
      process.kill(pid, 0);
      return true;
    } catch (e) {
      // ESRCH = No such process, EPERM = process exists but no permission
      return e.code === 'EPERM';
    }
  }

  /**
   * Get cluster status
   * @param {String} clusterId - Cluster ID
   * @returns {Object} Cluster status
   */
  getStatus(clusterId) {
    const cluster = this.clusters.get(clusterId);
    if (!cluster) {
      throw new Error(`Cluster ${clusterId} not found`);
    }

    // Detect zombie clusters: state=running but no backing process
    let state = cluster.state;
    let isZombie = false;
    if (state === 'running') {
      if (cluster.pid) {
        // PID recorded - check if process is running
        if (!this._isProcessRunning(cluster.pid)) {
          state = 'zombie';
          isZombie = true;
          this._log(
            `[Orchestrator] Detected zombie cluster ${clusterId} (PID ${cluster.pid} not running)`
          );
        }
      } else {
        // No PID recorded (legacy cluster or bug) - definitely a zombie
        // New code always records PID for running clusters
        state = 'zombie';
        isZombie = true;
        this._log(
          `[Orchestrator] Detected zombie cluster ${clusterId} (no PID recorded - legacy or killed cluster)`
        );
      }
    }

    return {
      id: clusterId,
      state: state,
      isZombie: isZombie,
      pid: cluster.pid || null,
      createdAt: cluster.createdAt,
      agents: cluster.agents.map((a) => a.getState()),
      messageCount: cluster.messageBus.count({ cluster_id: clusterId }),
    };
  }

  /**
   * List all clusters
   * @returns {Array} List of cluster summaries
   */
  listClusters() {
    return Array.from(this.clusters.values()).map((cluster) => {
      // Detect zombie clusters (state=running but no backing process)
      let state = cluster.state;
      if (state === 'running') {
        if (cluster.pid) {
          if (!this._isProcessRunning(cluster.pid)) {
            state = 'zombie';
          }
        } else {
          // No PID recorded - definitely a zombie
          state = 'zombie';
        }
      }

      return {
        id: cluster.id,
        state: state,
        createdAt: cluster.createdAt,
        agentCount: cluster.agents.length,
        messageCount: cluster.messageBus.getAll(cluster.id).length,
      };
    });
  }

  /**
   * Get cluster object (for internal use)
   * @param {String} clusterId - Cluster ID
   * @returns {Object} Full cluster object
   */
  getCluster(clusterId) {
    return this.clusters.get(clusterId);
  }

  /**
   * Export cluster conversation
   * @param {String} clusterId - Cluster ID
   * @param {String} format - Export format ('json' or 'markdown')
   * @returns {String} Exported data
   */
  export(clusterId, format = 'json') {
    const cluster = this.clusters.get(clusterId);
    if (!cluster) {
      throw new Error(`Cluster ${clusterId} not found`);
    }

    const messages = cluster.messageBus.getAll(clusterId);

    if (format === 'json') {
      return JSON.stringify(
        {
          cluster_id: clusterId,
          state: cluster.state,
          created_at: cluster.createdAt,
          agents: cluster.agents.map((a) => a.getState()),
          messages,
        },
        null,
        2
      );
    } else if (format === 'markdown') {
      return this._exportMarkdown(cluster, clusterId, messages);
    } else {
      throw new Error(`Unknown export format: ${format}`);
    }
  }

  /**
   * Export cluster as nicely formatted markdown
   * @private
   */
  _exportMarkdown(cluster, clusterId, messages) {
    const { parseProviderChunk } = require('./providers');

    // Find task info
    const issueOpened = messages.find((m) => m.topic === 'ISSUE_OPENED');
    const taskText = issueOpened?.content?.text || 'Unknown task';

    // Calculate duration
    const firstMsg = messages[0];
    const lastMsg = messages[messages.length - 1];
    const durationMs = lastMsg ? lastMsg.timestamp - firstMsg.timestamp : 0;
    const durationMin = Math.round(durationMs / 60000);

    // Header
    let md = `# Cluster: ${clusterId}\n\n`;
    md += `| Property | Value |\n|----------|-------|\n`;
    md += `| State | ${cluster.state} |\n`;
    md += `| Created | ${new Date(cluster.createdAt).toLocaleString()} |\n`;
    md += `| Duration | ${durationMin} minutes |\n`;
    md += `| Agents | ${cluster.agents.map((a) => a.id).join(', ')} |\n\n`;

    // Task
    md += `## Task\n\n${taskText}\n\n`;

    // Group messages by agent for cleaner output
    const agentOutputs = this._collectAgentOutputs(messages);

    for (const [agentId, agentMsgs] of agentOutputs) {
      md += this._renderAgentMarkdown(agentId, agentMsgs, parseProviderChunk);
    }

    md += this._renderValidationMarkdown(messages);

    // Final status
    const clusterComplete = messages.find((m) => m.topic === 'CLUSTER_COMPLETE');
    if (clusterComplete) {
      md += `## Result\n\n✅ **Cluster completed successfully**\n`;
    }

    return md;
  }

  _collectAgentOutputs(messages) {
    const agentOutputs = new Map();

    for (const msg of messages) {
      if (msg.topic !== 'AGENT_OUTPUT') continue;
      if (!agentOutputs.has(msg.sender)) {
        agentOutputs.set(msg.sender, []);
      }
      agentOutputs.get(msg.sender).push(msg);
    }

    return agentOutputs;
  }

  _extractAgentOutput(agentMsgs, parseProviderChunk) {
    let text = '';
    const tools = [];

    for (const msg of agentMsgs) {
      const content = msg.content?.data?.line || msg.content?.data?.chunk || msg.content?.text;
      if (!content) continue;

      const provider = normalizeProviderName(
        msg.content?.data?.provider || msg.sender_provider || 'claude'
      );
      const events = parseProviderChunk(provider, content);
      for (const event of events) {
        switch (event.type) {
          case 'text':
            text += event.text;
            break;
          case 'tool_call':
            tools.push({ name: event.toolName, input: event.input });
            break;
          case 'tool_result':
            if (tools.length > 0) {
              const lastTool = tools[tools.length - 1];
              lastTool.result = event.content;
              lastTool.isError = event.isError;
            }
            break;
        }
      }
    }

    return { text, tools };
  }

  _renderToolMarkdown(tools) {
    let md = `### Tools Used\n\n`;

    for (const tool of tools) {
      const status = tool.isError ? '❌' : '✓';
      md += `- **${tool.name}** ${status}\n`;
      if (tool.input) {
        const inputStr = typeof tool.input === 'string' ? tool.input : JSON.stringify(tool.input);
        if (inputStr.length < 100) {
          md += `  - Input: \`${inputStr}\`\n`;
        }
      }
    }

    return `${md}\n`;
  }

  _renderAgentMarkdown(agentId, agentMsgs, parseProviderChunk) {
    let md = `## Agent: ${agentId}\n\n`;
    const { text, tools } = this._extractAgentOutput(agentMsgs, parseProviderChunk);

    if (text.trim()) {
      md += `### Output\n\n${text.trim()}\n\n`;
    }

    if (tools.length > 0) {
      md += this._renderToolMarkdown(tools);
    }

    return md;
  }

  _renderCriteriaMarkdown(criteriaResults) {
    if (!Array.isArray(criteriaResults)) {
      return '';
    }

    let md = '';
    const cannotValidateYet = criteriaResults.filter((c) => c.status === 'CANNOT_VALIDATE_YET');
    if (cannotValidateYet.length > 0) {
      md += `**❌ Cannot Validate Yet (${cannotValidateYet.length} criteria - work incomplete):**\n`;
      for (const cv of cannotValidateYet) {
        md += `- ${cv.id}: ${cv.reason || 'No reason provided'}\n`;
      }
      md += '\n';
    }

    const cannotValidate = criteriaResults.filter((c) => c.status === 'CANNOT_VALIDATE');
    if (cannotValidate.length > 0) {
      md += `**⚠️ Could Not Validate (${cannotValidate.length} criteria - permanent):**\n`;
      for (const cv of cannotValidate) {
        md += `- ${cv.id}: ${cv.reason || 'No reason provided'}\n`;
      }
      md += '\n';
    }

    return md;
  }

  _renderValidationEntryMarkdown(validation) {
    const data = validation.content?.data || {};
    const approved = data.approved === true || data.approved === 'true';
    const icon = approved ? '✅' : '❌';
    let md = `### ${validation.sender} ${icon}\n\n`;

    if (data.summary) {
      md += `${data.summary}\n\n`;
    }

    if (!approved && data.issues) {
      const issues = typeof data.issues === 'string' ? JSON.parse(data.issues) : data.issues;
      if (Array.isArray(issues) && issues.length > 0) {
        md += `**Issues:**\n`;
        for (const issue of issues) {
          md += `- ${issue}\n`;
        }
        md += '\n';
      }
    }

    md += this._renderCriteriaMarkdown(data.criteriaResults);
    return md;
  }

  _renderValidationMarkdown(messages) {
    const validations = messages.filter((m) => m.topic === 'VALIDATION_RESULT');
    if (validations.length === 0) {
      return '';
    }

    let md = `## Validation Results\n\n`;
    for (const validation of validations) {
      md += this._renderValidationEntryMarkdown(validation);
    }

    return md;
  }

  /**
   * Validate cluster configuration (delegates to config-validator module)
   * @param {Object} config - Cluster configuration
   * @param {Object} options - Validation options
   * @param {boolean} options.strict - Treat warnings as errors (default: false)
   * @returns {Object} { valid: Boolean, errors: Array, warnings: Array }
   */
  validateConfig(config, options = {}) {
    const result = configValidator.validateConfig(config);

    // In strict mode, warnings become errors
    if (options.strict && result.warnings.length > 0) {
      result.errors.push(...result.warnings.map((w) => `[strict] ${w}`));
      result.valid = false;
    }

    return result;
  }

  /**
   * Load cluster configuration from file
   * @param {String} configPath - Path to config JSON file
   * @param {Object} options - Load options
   * @param {boolean} options.strict - Treat warnings as errors
   * @returns {Object} Parsed configuration
   */
  loadConfig(configPath, options = {}) {
    const fullPath = path.resolve(configPath);
    const content = fs.readFileSync(fullPath, 'utf8');
    const config = JSON.parse(content);

    const validation = this.validateConfig(config, options);

    // Show warnings (but don't fail unless strict mode)
    if (validation.warnings && validation.warnings.length > 0 && !this.quiet) {
      console.warn('\n⚠️  Configuration warnings:');
      for (const warning of validation.warnings) {
        console.warn(`   ${warning}`);
      }
      console.warn('');
    }

    if (!validation.valid) {
      const errorMsg = validation.errors.join('\n  ');
      throw new Error(`Invalid config:\n  ${errorMsg}`);
    }

    return config;
  }
}

module.exports = Orchestrator;
