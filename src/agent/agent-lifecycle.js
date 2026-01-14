// @ts-nocheck
/**
 * AgentLifecycle - Agent state machine and lifecycle management
 *
 * Provides:
 * - Agent startup and shutdown
 * - Message handling and routing
 * - Trigger action execution (execute_task, stop_cluster)
 * - Task execution with retry logic
 * - Liveness monitoring with multi-indicator stuck detection
 *
 * State machine: idle → evaluating → building_context → executing → idle
 */

const { buildContext } = require('./agent-context-builder');
const { findMatchingTrigger, evaluateTrigger } = require('./agent-trigger-evaluator');
const { executeHook } = require('./agent-hook-executor');
const {
  analyzeProcessHealth,
  isPlatformSupported,
  STUCK_THRESHOLD,
} = require('./agent-stuck-detector');

/**
 * Start the agent (begin listening for triggers)
 * @param {AgentWrapper} agent - Agent instance
 */
function start(agent) {
  if (agent.running) {
    throw new Error(`Agent ${agent.id} is already running`);
  }

  agent.running = true;
  agent.state = 'idle';

  // Subscribe to all messages for this cluster
  agent.unsubscribe = agent.messageBus.subscribe((message) => {
    if (message.cluster_id === agent.cluster.id) {
      handleMessage(agent, message).catch((error) => {
        // FATAL: Message handling failed - crash loud
        console.error(`\n${'='.repeat(80)}`);
        console.error(`🔴 FATAL: Agent ${agent.id} message handler crashed`);
        console.error(`${'='.repeat(80)}`);
        console.error(`Topic: ${message.topic}`);
        console.error(`Error: ${error.message}`);
        console.error(`Stack: ${error.stack}`);
        console.error(`${'='.repeat(80)}\n`);
        // Re-throw to crash the process - DO NOT SILENTLY CONTINUE
        throw error;
      });
    }
  });

  agent._log(`Agent ${agent.id} started (role: ${agent.role})`);
  agent._publishLifecycle('STARTED', {
    triggers: agent.config.triggers?.map((t) => t.topic) || [],
  });
}

/**
 * Stop the agent
 * Waits for any in-flight execution to complete before returning.
 * @param {AgentWrapper} agent - Agent instance
 * @returns {Promise<void>}
 */
async function stop(agent) {
  if (!agent.running) {
    return;
  }

  agent.running = false;
  agent.state = 'stopped';

  if (agent.unsubscribe) {
    agent.unsubscribe();
    agent.unsubscribe = null;
  }

  // Kill current task if any
  if (agent.currentTask) {
    agent._killTask();
  }

  // Wait for in-flight execution to complete (up to 5 seconds)
  // This prevents write-after-close race conditions
  if (agent._currentExecution) {
    try {
      await Promise.race([
        agent._currentExecution,
        new Promise((resolve) => setTimeout(resolve, 5000)),
      ]);
    } catch {
      // Ignore errors from cancelled execution
    }
    agent._currentExecution = null;
  }

  agent._log(`Agent ${agent.id} stopped`);
}

/**
 * Handle incoming message
 * @param {AgentWrapper} agent - Agent instance
 * @param {Object} message - Incoming message
 */
async function handleMessage(agent, message) {
  // Check if any trigger matches FIRST (before state check)
  const matchingTrigger = findMatchingTrigger({
    triggers: agent.config.triggers,
    message,
  });

  if (!matchingTrigger) {
    return; // No trigger for this message type
  }

  // Now check state - LOG if we're dropping a message we SHOULD handle
  if (!agent.running) {
    console.warn(`[${agent.id}] ⚠️ DROPPING message (not running): ${message.topic}`);
    return;
  }
  if (agent.state !== 'idle') {
    console.warn(
      `[${agent.id}] ⚠️ DROPPING message (busy, state=${agent.state}): ${message.topic}`
    );
    return;
  }

  // Evaluate trigger logic
  agent.state = 'evaluating_logic';

  const agentContext = {
    id: agent.id,
    role: agent.role,
    iteration: agent.iteration,
    cluster_id: agent.cluster.id,
  };

  const shouldExecute = evaluateTrigger({
    trigger: matchingTrigger,
    message,
    agent: agentContext,
    logicEngine: agent.logicEngine,
  });

  if (!shouldExecute) {
    agent.state = 'idle';
    return;
  }

  // Execute trigger action (lifecycle event published inside for execute_task)
  // Track execution so stop() can wait for it
  const executionPromise = executeTriggerAction(agent, matchingTrigger, message);
  agent._currentExecution = executionPromise;
  try {
    await executionPromise;
  } finally {
    // Clear only if this is still our execution (not replaced by another)
    if (agent._currentExecution === executionPromise) {
      agent._currentExecution = null;
    }
  }
}

/**
 * Execute trigger action
 * @param {AgentWrapper} agent - Agent instance
 * @param {Object} trigger - Matched trigger config
 * @param {Object} message - Triggering message
 */
async function executeTriggerAction(agent, trigger, message) {
  const action = trigger.action || 'execute_task';

  if (action === 'execute_task') {
    await executeTask(agent, message);
  } else if (action === 'stop_cluster') {
    // Publish CLUSTER_COMPLETE message to signal successful completion
    agent._publish({
      topic: 'CLUSTER_COMPLETE',
      receiver: 'system',
      content: {
        text: 'All validation passed. Cluster completing successfully.',
        data: {
          reason: 'all_validators_approved',
          timestamp: Date.now(),
        },
      },
    });
    agent.state = 'completed';
    agent._log(`Agent ${agent.id}: Cluster completion triggered`);
  } else {
    console.warn(`Unknown action: ${action}`);
    agent.state = 'idle';
  }
}

/**
 * Execute claude-zeroshots with built context
 * Retries disabled by default. Set agent config `maxRetries` to enable (e.g., 3).
 * @param {AgentWrapper} agent - Agent instance
 * @param {Object} triggeringMessage - Message that triggered execution
 */
async function executeTask(agent, triggeringMessage) {
  // Early exit if agent was stopped
  if (!agent.running) {
    return;
  }

  // Default: no retries (maxRetries=1 means 1 attempt only)
  // Set agent config `maxRetries: 3` to enable exponential backoff retries
  const maxRetries = agent.config.maxRetries ?? 1;
  const baseDelay = 2000; // 2 seconds

  for (let attempt = 1; attempt <= maxRetries; attempt++) {
    // Check if agent was stopped between retries
    if (!agent.running) {
      return;
    }

    try {
      // Execute onStart hook
      await executeHook({
        hook: agent.config.hooks?.onStart,
        agent: agent,
        message: triggeringMessage,
        result: undefined,
        messageBus: agent.messageBus,
        cluster: agent.cluster,
        orchestrator: agent.orchestrator,
      });

      // Check max iterations limit BEFORE incrementing (prevents infinite rejection loops)
      if (agent.iteration >= agent.maxIterations) {
        agent._log(
          `[Agent ${agent.id}] Hit max iterations (${agent.maxIterations}), stopping cluster`
        );
        agent._publishLifecycle('MAX_ITERATIONS_REACHED', {
          iteration: agent.iteration,
          maxIterations: agent.maxIterations,
        });
        // Publish failure message - orchestrator watches for this and auto-stops
        agent._publish({
          topic: 'CLUSTER_FAILED',
          receiver: 'system',
          content: {
            text: `Agent ${agent.id} hit max iterations limit (${agent.maxIterations}). Stopping cluster.`,
            data: {
              reason: 'max_iterations',
              iteration: agent.iteration,
              maxIterations: agent.maxIterations,
            },
          },
        });
        agent.state = 'failed';
        return;
      }

      // Increment iteration BEFORE building context so worker knows current iteration
      agent.iteration++;

      // Build context
      agent.state = 'building_context';
      const context = buildContext({
        id: agent.id,
        role: agent.role,
        iteration: agent.iteration,
        config: agent.config,
        messageBus: agent.messageBus,
        cluster: agent.cluster,
        lastTaskEndTime: agent.lastTaskEndTime,
        triggeringMessage,
        selectedPrompt: agent._selectPrompt(),
      });

      // Log input context (helps debug what each agent sees)
      if (!agent.quiet) {
        console.log(`\n${'='.repeat(80)}`);
        console.log(`📥 INPUT CONTEXT - Agent: ${agent.id} (Iteration: ${agent.iteration})`);
        console.log(`${'='.repeat(80)}`);
        console.log(context);
        console.log(`${'='.repeat(80)}\n`);
      }

      // Spawn provider task
      agent.state = 'executing_task';

      // LOCK CONTENTION FIX: Add random jitter for validators to prevent thundering herd
      // When multiple validators wake on the same trigger (e.g., IMPLEMENTATION_READY),
      // they all try to spawn Claude CLI at the same time. Claude CLI uses a lock file
      // per workspace, so only one can run. Adding jitter staggers their starts.
      // SKIP in testMode - tests use mocks and don't need jitter
      if (agent.role === 'validator' && !agent.testMode) {
        const jitterMs = Math.floor(Math.random() * 15000); // 0-15 seconds
        if (!agent.quiet) {
          agent._log(
            `[Agent ${agent.id}] Adding ${Math.round(jitterMs / 1000)}s jitter to prevent lock contention`
          );
        }
        await new Promise((resolve) => setTimeout(resolve, jitterMs));
      }

      const modelSpec = agent._resolveModelSpec ? agent._resolveModelSpec() : null;
      agent._publishLifecycle('TASK_STARTED', {
        iteration: agent.iteration,
        model: agent._selectModel(),
        provider: agent._resolveProvider ? agent._resolveProvider() : 'claude',
        modelSpec,
        triggeredBy: triggeringMessage.topic,
        triggerFrom: triggeringMessage.sender,
      });

      const result = await agent._spawnClaudeTask(context);

      // Add task ID to result for debugging and hooks
      result.taskId = agent.currentTaskId;
      result.agentId = agent.id;
      result.iteration = agent.iteration;

      // Check if task execution failed
      if (!result.success) {
        throw new Error(result.error || 'Task execution failed');
      }

      // Set state to idle BEFORE publishing lifecycle event
      // (so lifecycle message includes correct state)
      agent.state = 'idle';

      // Track completion time for context filtering (used by "since: last_task_end")
      agent.lastTaskEndTime = Date.now();

      agent._publishLifecycle('TASK_COMPLETED', {
        iteration: agent.iteration,
        success: true,
        taskId: agent.currentTaskId,
        tokenUsage: result.tokenUsage || null,
      });

      // Publish TOKEN_USAGE event for aggregation and tracking
      // CRITICAL: Include taskId for causal linking - allows consumers to group
      // messages by task regardless of interleaved timing from async hooks
      if (result.tokenUsage) {
        // Get actual model used from API response (more accurate than config)
        const actualModel = result.tokenUsage.modelUsage
          ? Object.keys(result.tokenUsage.modelUsage)[0]
          : agent._selectModel();

        agent.messageBus.publish({
          cluster_id: agent.cluster.id,
          topic: 'TOKEN_USAGE',
          sender: agent.id,
          content: {
            text: `${agent.id} used ${result.tokenUsage.inputTokens} input + ${result.tokenUsage.outputTokens} output tokens (${actualModel})`,
            data: {
              agentId: agent.id,
              role: agent.role,
              model: agent._selectModel(),
              iteration: agent.iteration,
              taskId: agent.currentTaskId, // Causal linking for message ordering
              ...result.tokenUsage,
            },
          },
        });
      }

      // Execute onComplete hook WITH RETRY
      // Hook failure shouldn't retry the entire task - just the hook
      const hookMaxRetries = 3;
      const hookBaseDelay = 1000;
      let hookSuccess = false;

      for (let hookAttempt = 1; hookAttempt <= hookMaxRetries && !hookSuccess; hookAttempt++) {
        try {
          await executeHook({
            hook: agent.config.hooks?.onComplete,
            agent: agent,
            message: triggeringMessage,
            result: result,
            messageBus: agent.messageBus,
            cluster: agent.cluster,
            orchestrator: agent.orchestrator,
          });
          hookSuccess = true;
        } catch (hookError) {
          console.error(`\n${'='.repeat(80)}`);
          console.error(
            `🔴 HOOK EXECUTION FAILED - AGENT: ${agent.id} (Attempt ${hookAttempt}/${hookMaxRetries})`
          );
          console.error(`${'='.repeat(80)}`);
          console.error(`Error: ${hookError.message}`);

          if (hookAttempt < hookMaxRetries) {
            const delay = hookBaseDelay * Math.pow(2, hookAttempt - 1);
            console.error(`Will retry hook in ${delay}ms...`);
            console.error(`${'='.repeat(80)}\n`);
            await new Promise((resolve) => setTimeout(resolve, delay));
          } else {
            console.error(`${'='.repeat(80)}\n`);
            // All hook retries exhausted - throw to trigger task-level handling
            throw new Error(
              `Hook execution failed after ${hookMaxRetries} attempts. ` +
                `Task completed successfully but hook could not publish result. ` +
                `Original error: ${hookError.message}`
            );
          }
        }
      }

      // ✅ SUCCESS - exit retry loop
      return;
    } catch (error) {
      // LOCK CONTENTION: Add extra jittered delay for lock file errors
      // This happens when multiple validators try to run Claude CLI in the same workspace
      const isLockError = error.message && error.message.includes('Lock file');

      // Log attempt failure
      console.error(`\n${'='.repeat(80)}`);
      console.error(
        `🔴 TASK EXECUTION FAILED - AGENT: ${agent.id} (Attempt ${attempt}/${maxRetries})`
      );
      console.error(`${'='.repeat(80)}`);
      console.error(`Error: ${error.message}`);

      if (isLockError) {
        // Lock contention - add significant jittered delay
        const lockDelay = 10000 + Math.floor(Math.random() * 20000); // 10-30 seconds
        console.error(
          `⚠️ Lock contention detected - waiting ${Math.round(lockDelay / 1000)}s before retry`
        );
        await new Promise((resolve) => setTimeout(resolve, lockDelay));
      } else if (attempt < maxRetries) {
        console.error(`Will retry in ${baseDelay * Math.pow(2, attempt - 1)}ms...`);
      }
      console.error(`${'='.repeat(80)}\n`);

      // Last attempt - give up
      if (attempt >= maxRetries) {
        console.error(`\n${'='.repeat(80)}`);
        console.error(`🔴🔴🔴 MAX RETRIES EXHAUSTED - AGENT: ${agent.id} 🔴🔴🔴`);
        console.error(`${'='.repeat(80)}`);
        console.error(`All ${maxRetries} attempts failed`);
        console.error(`Final error: ${error.message}`);
        console.error(`Stack: ${error.stack}`);
        console.error(`${'='.repeat(80)}\n`);

        // CRITICAL FIX: Validator crash = REJECTION (not auto-approval)
        // Auto-approval on crash allowed broken code to be merged - unacceptable!
        // If validator crashed 3x, something is fundamentally wrong - REJECT and investigate
        if (agent.role === 'validator') {
          console.error(`\n${'='.repeat(80)}`);
          console.error(`❌ VALIDATOR CRASHED - REJECTING (NOT AUTO-APPROVING)`);
          console.error(`${'='.repeat(80)}`);
          console.error(`Validator ${agent.id} crashed ${maxRetries} times`);
          console.error(`Error: ${error.message}`);
          console.error(`REJECTING validation - broken code will NOT be merged`);
          console.error(`Investigation required before retry`);
          console.error(`${'='.repeat(80)}\n`);

          // Publish REJECTION message (NOT approval!)
          const hook = agent.config.hooks?.onComplete;
          if (hook && hook.action === 'publish_message') {
            agent._publish({
              topic: hook.config.topic,
              receiver: hook.config.receiver || 'broadcast',
              content: {
                text: `REJECTED: Validator crashed ${maxRetries} times - ${error.message}`,
                data: {
                  approved: false, // REJECT!
                  crashedAfterRetries: true,
                  errors: JSON.stringify([
                    `VALIDATOR CRASHED ${maxRetries}x: ${error.message}`,
                    `Validation could not be performed - REJECTING to prevent broken code merge`,
                    `Investigation required before retry`,
                  ]),
                  attempts: maxRetries,
                  requiresInvestigation: true,
                },
              },
            });
          }

          agent.state = 'error';
          // Don't return - fall through to publish AGENT_ERROR and save failure info
          // This allows the cluster to stop and be resumed after investigation
        }

        // Non-validator agents: publish error and stop
        agent.state = 'error';

        // Save failure info to cluster for resume capability
        agent.cluster.failureInfo = {
          agentId: agent.id,
          taskId: agent.currentTaskId,
          iteration: agent.iteration,
          error: error.message,
          attempts: maxRetries,
          timestamp: Date.now(),
        };

        // Publish error to message bus for visibility in logs
        agent._publish({
          topic: 'AGENT_ERROR',
          receiver: 'broadcast',
          content: {
            text: `Task execution failed after ${maxRetries} attempts: ${error.message}`,
            data: {
              error: error.message,
              stack: error.stack,
              agent: agent.id,
              role: agent.role,
              iteration: agent.iteration,
              taskId: agent.currentTaskId,
              attempts: maxRetries,
              hookFailureContext: error.message.includes('Hook uses result')
                ? {
                    taskId: agent.currentTaskId || 'UNKNOWN',
                    retrieveLogs: agent.currentTaskId
                      ? `zeroshot task logs ${agent.currentTaskId}`
                      : 'N/A',
                  }
                : undefined,
            },
          },
          metadata: {
            triggeringTopic: triggeringMessage.topic,
          },
        });

        // Execute onError hook
        await executeHook({
          hook: agent.config.hooks?.onError,
          agent: agent,
          message: triggeringMessage,
          result: { error },
          messageBus: agent.messageBus,
          cluster: agent.cluster,
          orchestrator: agent.orchestrator,
        });

        agent.state = 'idle';
        return; // Give up
      }

      // Not the last attempt - prepare for retry
      const delay = baseDelay * Math.pow(2, attempt - 1); // 2s, 4s, 8s

      agent._publishLifecycle('RETRY_SCHEDULED', {
        attempt,
        maxRetries,
        delayMs: delay,
        error: error.message,
      });

      agent._log(`[${agent.id}] ⚠️  Retrying in ${delay}ms... (${attempt + 1}/${maxRetries})`);

      // Exponential backoff
      await new Promise((resolve) => setTimeout(resolve, delay));

      agent._log(`[${agent.id}] 🔄 Starting retry attempt ${attempt + 1}/${maxRetries}`);
      // Continue to next iteration of for loop
    }
  }
}

/**
 * Start monitoring agent output liveness using multi-indicator stuck detection
 *
 * SAFE DETECTION: Only flags as stuck when MULTIPLE indicators agree:
 * - Process sleeping (state=S)
 * - Blocked on epoll/poll wait
 * - Low CPU usage (<1%)
 * - Low context switches (<10)
 * - No network data in flight
 *
 * Single-indicator detection (just output freshness) has HIGH false positive risk.
 * This multi-indicator approach eliminates false positives.
 *
 * @param {AgentWrapper} agent - Agent instance
 */
function startLivenessCheck(agent) {
  if (agent.livenessCheckInterval) {
    clearInterval(agent.livenessCheckInterval);
  }

  // Check if platform supports /proc filesystem (Linux only)
  if (!isPlatformSupported()) {
    agent._log(
      `[${agent.id}] Liveness check disabled: /proc filesystem not available (non-Linux platform)`
    );
    return;
  }

  // Check every 60 seconds (gives time for multi-indicator analysis)
  const CHECK_INTERVAL_MS = 60 * 1000;
  const ANALYSIS_SAMPLE_MS = 5000; // Sample CPU/context switches over 5 seconds

  agent.livenessCheckInterval = setInterval(async () => {
    // Skip if no task running or no PID tracked
    if (!agent.currentTask || !agent.processPid) {
      return;
    }

    // Skip if output is recent (process is clearly active)
    if (agent.lastOutputTime) {
      const timeSinceLastOutput = Date.now() - agent.lastOutputTime;
      if (timeSinceLastOutput < agent.staleDuration) {
        return; // Output is recent, definitely not stuck
      }
    }

    // Output is stale - run multi-indicator analysis to confirm
    agent._log(
      `[${agent.id}] Output stale for ${Math.round((Date.now() - (agent.lastOutputTime || 0)) / 1000)}s, running multi-indicator analysis...`
    );

    try {
      const analysis = await analyzeProcessHealth(agent.processPid, ANALYSIS_SAMPLE_MS);

      // Process died during analysis
      if (analysis.isLikelyStuck === null) {
        agent._log(`[${agent.id}] Process analysis inconclusive: ${analysis.reason}`);
        return;
      }

      // Log analysis details for debugging
      agent._log(
        `[${agent.id}] Analysis: score=${analysis.stuckScore}/${STUCK_THRESHOLD}, ` +
          `state=${analysis.state}, wchan=${analysis.wchan}, ` +
          `CPU=${analysis.cpuPercent}%, ctxSwitches=${analysis.ctxSwitchesDelta}`
      );

      if (analysis.isLikelyStuck) {
        agent._log(`⚠️  Agent ${agent.id}: CONFIRMED STUCK (confidence: ${analysis.confidence})`);
        agent._log(`    ${analysis.analysis}`);

        // CHANGED: Stale detection is informational only - never kills tasks
        // Publish stale detection event with full analysis (for logging/monitoring)
        agent._publishLifecycle('AGENT_STALE_WARNING', {
          timeSinceLastOutput: Date.now() - (agent.lastOutputTime || 0),
          staleDuration: agent.staleDuration,
          lastOutputTime: agent.lastOutputTime,
          // Multi-indicator analysis results
          stuckScore: analysis.stuckScore,
          confidence: analysis.confidence,
          processState: analysis.state,
          wchan: analysis.wchan,
          cpuPercent: analysis.cpuPercent,
          ctxSwitchesDelta: analysis.ctxSwitchesDelta,
          indicators: analysis.indicators,
          analysis: analysis.analysis,
        });

        // Keep monitoring - do NOT stop the agent
        // User can manually intervene with 'zeroshot resume' if needed
        // stopLivenessCheck(agent); // REMOVED - keep monitoring
      } else {
        agent._log(
          `[${agent.id}] Process appears WORKING despite stale output (score: ${analysis.stuckScore})`
        );
        agent._log(`    ${analysis.analysis}`);
        // Don't flag as stuck - process is legitimately working
      }
    } catch (err) {
      agent._log(`[${agent.id}] Error during stuck analysis: ${err.message}`);
      // Don't flag as stuck on analysis error
    }
  }, CHECK_INTERVAL_MS);
}

/**
 * Stop liveness monitoring
 * @param {AgentWrapper} agent - Agent instance
 */
function stopLivenessCheck(agent) {
  if (agent.livenessCheckInterval) {
    clearInterval(agent.livenessCheckInterval);
    agent.livenessCheckInterval = null;
  }
}

module.exports = {
  start,
  stop,
  handleMessage,
  executeTriggerAction,
  executeTask,
  startLivenessCheck,
  stopLivenessCheck,
};
