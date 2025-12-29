/**
 * Config Validator - Static analysis for zeroshot cluster configurations
 *
 * Catches logical failures that would cause clusters to:
 * - Never start (no bootstrap trigger)
 * - Never complete (no path to completion)
 * - Loop infinitely (circular dependencies)
 * - Deadlock (impossible consensus)
 * - Waste compute (orchestrator executing tasks)
 *
 * Run at config load time to fail fast before spawning agents.
 */

/**
 * Check if config is a conductor-bootstrap style config
 * Conductor configs dynamically spawn agents via CLUSTER_OPERATIONS
 * @param {Object} config - Cluster configuration
 * @returns {boolean}
 */
function isConductorConfig(config) {
  return config.agents?.some(
    (a) =>
      a.role === 'conductor' &&
      // Old style: static topic in config
      (a.hooks?.onComplete?.config?.topic === 'CLUSTER_OPERATIONS' ||
        // New style: topic set in transform script (check for CLUSTER_OPERATIONS in script)
        a.hooks?.onComplete?.transform?.script?.includes('CLUSTER_OPERATIONS'))
  );
}

/**
 * Validate a cluster configuration for structural correctness
 * @param {Object} config - Cluster configuration
 * @param {Number} depth - Current nesting depth (for subcluster validation)
 * @returns {{ valid: boolean, errors: string[], warnings: string[] }}
 */
function validateConfig(config, depth = 0) {
  const errors = [];
  const warnings = [];

  // Max nesting depth check
  const MAX_DEPTH = 5;
  if (depth > MAX_DEPTH) {
    errors.push(`Cluster nesting exceeds max depth (${MAX_DEPTH})`);
    return { valid: false, errors, warnings };
  }

  // === PHASE 1: Basic structure validation ===
  const basicResult = validateBasicStructure(config, depth);
  errors.push(...basicResult.errors);
  warnings.push(...basicResult.warnings);
  if (basicResult.errors.length > 0) {
    // Can't proceed with flow analysis if basic structure is broken
    return { valid: false, errors, warnings };
  }

  // Conductor configs dynamically spawn agents - skip message flow analysis
  // The orchestrator validates the spawned config at CLUSTER_OPERATIONS execution time
  const conductorMode = isConductorConfig(config);

  // === PHASE 2: Message flow analysis (skip for conductor configs) ===
  if (!conductorMode) {
    const flowResult = analyzeMessageFlow(config);
    errors.push(...flowResult.errors);
    warnings.push(...flowResult.warnings);
  }

  // === PHASE 3: Agent-specific validation ===
  const agentResult = validateAgents(config);
  errors.push(...agentResult.errors);
  warnings.push(...agentResult.warnings);

  // === PHASE 4: Logic script validation ===
  const logicResult = validateLogicScripts(config);
  errors.push(...logicResult.errors);
  warnings.push(...logicResult.warnings);

  // === PHASE 5: Template variable validation ===
  const templateResult = validateTemplateVariables(config, depth);
  errors.push(...templateResult.errors);
  warnings.push(...templateResult.warnings);

  return {
    valid: errors.length === 0,
    errors,
    warnings,
  };
}

/**
 * Phase 1: Validate basic structure (fields, types, duplicates)
 */
function validateBasicStructure(config, depth = 0) {
  const errors = [];
  const warnings = [];

  if (!config.agents || !Array.isArray(config.agents)) {
    errors.push('agents array is required');
    return { errors, warnings };
  }

  if (config.agents.length === 0) {
    errors.push('agents array cannot be empty');
    return { errors, warnings };
  }

  const seenIds = new Set();

  for (let i = 0; i < config.agents.length; i++) {
    const agent = config.agents[i];
    const prefix = `agents[${i}]`;

    // Check if this is a subcluster
    const isSubCluster = agent.type === 'subcluster';

    // Required fields
    if (!agent.id) {
      errors.push(`${prefix}.id is required`);
    } else if (typeof agent.id !== 'string') {
      errors.push(`${prefix}.id must be a string`);
    } else if (seenIds.has(agent.id)) {
      errors.push(`Duplicate agent id: "${agent.id}"`);
    } else {
      seenIds.add(agent.id);
    }

    if (!agent.role) {
      errors.push(`${prefix}.role is required`);
    }

    // Validate subclusters
    if (isSubCluster) {
      const subClusterSchema = require('./schemas/sub-cluster');
      const subResult = subClusterSchema.validateSubCluster(agent, depth);
      errors.push(...subResult.errors);
      warnings.push(...subResult.warnings);
      continue; // Skip regular agent validation
    }

    // Regular agent validation
    if (!agent.triggers || !Array.isArray(agent.triggers)) {
      errors.push(`${prefix}.triggers array is required`);
    } else if (agent.triggers.length === 0) {
      errors.push(`${prefix}.triggers cannot be empty (agent would never activate)`);
    }

    // Validate triggers structure
    if (agent.triggers) {
      for (let j = 0; j < agent.triggers.length; j++) {
        const trigger = agent.triggers[j];
        const triggerPrefix = `${prefix}.triggers[${j}]`;

        if (!trigger.topic) {
          errors.push(`${triggerPrefix}.topic is required`);
        }

        if (trigger.action && !['execute_task', 'stop_cluster'].includes(trigger.action)) {
          errors.push(
            `${triggerPrefix}.action must be 'execute_task' or 'stop_cluster', got '${trigger.action}'`
          );
        }

        if (trigger.logic) {
          if (!trigger.logic.script) {
            errors.push(`${triggerPrefix}.logic.script is required when logic is specified`);
          }
          if (trigger.logic.engine && trigger.logic.engine !== 'javascript') {
            errors.push(
              `${triggerPrefix}.logic.engine must be 'javascript', got '${trigger.logic.engine}'`
            );
          }
        }
      }
    }

    // Validate model rules if present
    if (agent.modelRules) {
      if (!Array.isArray(agent.modelRules)) {
        errors.push(`${prefix}.modelRules must be an array`);
      } else {
        for (let j = 0; j < agent.modelRules.length; j++) {
          const rule = agent.modelRules[j];
          const rulePrefix = `${prefix}.modelRules[${j}]`;

          if (!rule.iterations) {
            errors.push(`${rulePrefix}.iterations is required`);
          } else if (!isValidIterationPattern(rule.iterations)) {
            errors.push(
              `${rulePrefix}.iterations '${rule.iterations}' is invalid. Valid: "1", "1-3", "5+", "all"`
            );
          }

          if (!rule.model) {
            errors.push(`${rulePrefix}.model is required`);
          } else if (!['opus', 'sonnet', 'haiku'].includes(rule.model)) {
            errors.push(
              `${rulePrefix}.model must be 'opus', 'sonnet', or 'haiku', got '${rule.model}'`
            );
          }
        }

        // Check for coverage gap (no catch-all rule)
        const hasCatchAll = agent.modelRules.some(
          (r) => r.iterations === 'all' || /^\d+\+$/.test(r.iterations)
        );
        if (!hasCatchAll) {
          errors.push(
            `${prefix}.modelRules has no catch-all rule (e.g., "all" or "5+"). High iterations will fail.`
          );
        }
      }
    }
  }

  return { errors, warnings };
}

/**
 * Phase 2: Analyze message flow for structural problems
 */
function analyzeMessageFlow(config) {
  const errors = [];
  const warnings = [];

  // Build topic graph
  const topicProducers = new Map(); // topic -> [agentIds that produce it]
  const topicConsumers = new Map(); // topic -> [agentIds that consume it]
  const agentOutputTopics = new Map(); // agentId -> [topics it produces]
  const agentInputTopics = new Map(); // agentId -> [topics it consumes]

  // System always produces ISSUE_OPENED
  topicProducers.set('ISSUE_OPENED', ['system']);

  for (const agent of config.agents) {
    agentInputTopics.set(agent.id, []);
    agentOutputTopics.set(agent.id, []);

    // Track what topics this agent consumes (triggers)
    for (const trigger of agent.triggers || []) {
      const topic = trigger.topic;
      if (!topicConsumers.has(topic)) {
        topicConsumers.set(topic, []);
      }
      topicConsumers.get(topic).push(agent.id);
      agentInputTopics.get(agent.id).push(topic);
    }

    // Track what topics this agent produces (hooks)
    const outputTopic = agent.hooks?.onComplete?.config?.topic;
    if (outputTopic) {
      if (!topicProducers.has(outputTopic)) {
        topicProducers.set(outputTopic, []);
      }
      topicProducers.get(outputTopic).push(agent.id);
      agentOutputTopics.get(agent.id).push(outputTopic);
    }
  }

  // === CHECK 1: No bootstrap trigger ===
  const issueOpenedConsumers = topicConsumers.get('ISSUE_OPENED') || [];
  if (issueOpenedConsumers.length === 0) {
    errors.push(
      'No agent triggers on ISSUE_OPENED. Cluster will never start. ' +
        'Add a trigger: { "topic": "ISSUE_OPENED", "action": "execute_task" }'
    );
  }

  // === CHECK 2: No completion handler ===
  const completionHandlers = config.agents.filter(
    (a) =>
      a.triggers?.some((t) => t.action === 'stop_cluster') ||
      a.id === 'completion-detector' ||
      a.id === 'git-pusher' ||
      a.hooks?.onComplete?.config?.topic === 'CLUSTER_COMPLETE'
  );

  if (completionHandlers.length === 0) {
    errors.push(
      'No completion handler found. Cluster will run until idle timeout (2 min). ' +
        'Add an agent with trigger action: "stop_cluster"'
    );
  } else if (completionHandlers.length > 1) {
    errors.push(
      `Multiple completion handlers: [${completionHandlers.map((a) => a.id).join(', ')}]. ` +
        'This causes race conditions. Keep only one.'
    );
  }

  // === CHECK 3: Orphan topics (produced but never consumed) ===
  for (const [topic, producers] of topicProducers) {
    if (topic === 'CLUSTER_COMPLETE') continue; // System handles this
    const consumers = topicConsumers.get(topic) || [];
    if (consumers.length === 0) {
      warnings.push(
        `Topic '${topic}' is produced by [${producers.join(', ')}] but never consumed. Dead end.`
      );
    }
  }

  // === CHECK 4: Waiting for topics that are never produced ===
  for (const [topic, consumers] of topicConsumers) {
    if (topic === 'ISSUE_OPENED' || topic === 'CLUSTER_RESUMED') continue; // System produces
    if (topic.endsWith('*')) continue; // Wildcard pattern
    const producers = topicProducers.get(topic) || [];
    if (producers.length === 0) {
      errors.push(
        `Topic '${topic}' consumed by [${consumers.join(', ')}] but never produced. ` +
          'These agents will never trigger.'
      );
    }
  }

  // === CHECK 5: Self-triggering agents (instant infinite loop) ===
  for (const agent of config.agents) {
    const inputs = agentInputTopics.get(agent.id) || [];
    const outputs = agentOutputTopics.get(agent.id) || [];
    const selfTrigger = inputs.find((t) => outputs.includes(t));
    if (selfTrigger) {
      errors.push(
        `Agent '${agent.id}' triggers on '${selfTrigger}' and produces '${selfTrigger}'. ` +
          'Instant infinite loop.'
      );
    }
  }

  // === CHECK 6: Two-agent circular dependency ===
  for (const agentA of config.agents) {
    const outputsA = agentOutputTopics.get(agentA.id) || [];
    for (const agentB of config.agents) {
      if (agentA.id === agentB.id) continue;
      const inputsB = agentInputTopics.get(agentB.id) || [];
      const outputsB = agentOutputTopics.get(agentB.id) || [];
      const inputsA = agentInputTopics.get(agentA.id) || [];

      // A produces what B consumes, AND B produces what A consumes
      const aToB = outputsA.some((t) => inputsB.includes(t));
      const bToA = outputsB.some((t) => inputsA.includes(t));

      if (aToB && bToA) {
        // This might be intentional (rejection loop), check if there's an escape
        const hasEscapeLogic =
          agentA.triggers?.some((t) => t.logic) || agentB.triggers?.some((t) => t.logic);
        if (!hasEscapeLogic) {
          warnings.push(
            `Circular dependency: '${agentA.id}' ↔ '${agentB.id}'. ` +
              'Add logic conditions to prevent infinite loop, or ensure maxIterations is set.'
          );
        }
      }
    }
  }

  // === CHECK 7: Validator without worker re-trigger ===
  const validators = config.agents.filter((a) => a.role === 'validator');
  const workers = config.agents.filter((a) => a.role === 'implementation');

  if (validators.length > 0 && workers.length > 0) {
    for (const worker of workers) {
      const triggersOnValidation = worker.triggers?.some(
        (t) => t.topic === 'VALIDATION_RESULT' || t.topic.includes('VALIDATION')
      );
      if (!triggersOnValidation) {
        errors.push(
          `Worker '${worker.id}' has validators but doesn't trigger on VALIDATION_RESULT. ` +
            'Rejections will be ignored. Add trigger: { "topic": "VALIDATION_RESULT", "logic": {...} }'
        );
      }
    }
  }

  // === CHECK 8: Context strategy missing trigger topics ===
  for (const agent of config.agents) {
    if (!agent.contextStrategy?.sources) continue;

    const triggerTopics = (agent.triggers || []).map((t) => t.topic);
    const contextTopics = agent.contextStrategy.sources.map((s) => s.topic);

    for (const triggerTopic of triggerTopics) {
      if (triggerTopic === 'ISSUE_OPENED' || triggerTopic === 'CLUSTER_RESUMED') continue;
      if (triggerTopic.endsWith('*')) continue;

      if (!contextTopics.includes(triggerTopic)) {
        warnings.push(
          `Agent '${agent.id}' triggers on '${triggerTopic}' but doesn't include it in contextStrategy. ` +
            'Agent may not see what triggered it.'
        );
      }
    }
  }

  return { errors, warnings };
}

/**
 * Phase 3: Validate agent-specific configurations
 */
function validateAgents(config) {
  const errors = [];
  const warnings = [];

  const roles = new Map(); // role -> [agentIds]

  for (const agent of config.agents) {
    // Track roles
    if (!roles.has(agent.role)) {
      roles.set(agent.role, []);
    }
    roles.get(agent.role).push(agent.id);

    // Orchestrator should not execute tasks
    if (agent.role === 'orchestrator') {
      const executesTask = agent.triggers?.some(
        (t) => t.action === 'execute_task' || (!t.action && !t.logic)
      );
      if (executesTask) {
        warnings.push(
          `Orchestrator '${agent.id}' has execute_task triggers. ` +
            'Orchestrators typically use action: "stop_cluster". This may waste API calls.'
        );
      }
    }

    // Check for git operations in validator prompts (unreliable in agents)
    if (agent.role === 'validator') {
      const prompt = typeof agent.prompt === 'string' ? agent.prompt : agent.prompt?.system;
      const gitPatterns = ['git diff', 'git status', 'git log', 'git show'];
      for (const pattern of gitPatterns) {
        if (prompt?.includes(pattern)) {
          errors.push(
            `Validator '${agent.id}' uses '${pattern}' - git state is unreliable in agents`
          );
        }
      }
    }

    // JSON output without schema
    if (agent.outputFormat === 'json' && !agent.jsonSchema) {
      warnings.push(
        `Agent '${agent.id}' has outputFormat: 'json' but no jsonSchema. ` +
          'Output parsing may be unreliable.'
      );
    }

    // Very high maxIterations
    if (agent.maxIterations && agent.maxIterations > 50) {
      warnings.push(
        `Agent '${agent.id}' has maxIterations: ${agent.maxIterations}. ` +
          'This may consume significant API credits if stuck in a loop.'
      );
    }

    // No maxIterations on implementation agent (unbounded retries)
    if (agent.role === 'implementation' && !agent.maxIterations) {
      warnings.push(
        `Implementation agent '${agent.id}' has no maxIterations. ` +
          'Defaults to 30, but consider setting explicitly.'
      );
    }
  }

  // Check for role references in logic scripts
  // IMPORTANT: Changed from error to warning because some triggers are designed to be
  // no-ops when the referenced role doesn't exist (e.g., worker's VALIDATION_RESULT
  // trigger returns false when validators.length === 0)
  for (const agent of config.agents) {
    for (const trigger of agent.triggers || []) {
      if (trigger.logic?.script) {
        const script = trigger.logic.script;
        const roleMatch = script.match(/getAgentsByRole\(['"](\w+)['"]\)/g);
        if (roleMatch) {
          for (const match of roleMatch) {
            const role = match.match(/['"](\w+)['"]/)[1];
            if (!roles.has(role)) {
              warnings.push(
                `Agent '${agent.id}' logic references role '${role}' but no agent has that role. ` +
                  `Trigger may be a no-op. Available roles: [${Array.from(roles.keys()).join(', ')}]`
              );
            }
          }
        }
      }
    }
  }

  return { errors, warnings };
}

/**
 * Phase 4: Validate logic scripts (syntax only, not semantics)
 */
function validateLogicScripts(config) {
  const errors = [];
  const warnings = [];

  const vm = require('vm');

  for (const agent of config.agents) {
    for (const trigger of agent.triggers || []) {
      if (!trigger.logic?.script) continue;

      const script = trigger.logic.script;

      // Syntax check
      try {
        const wrappedScript = `(function() { ${script} })()`;
        new vm.Script(wrappedScript);
      } catch (syntaxError) {
        errors.push(`Agent '${agent.id}' has invalid logic script: ${syntaxError.message}`);
        continue;
      }

      // Check for common mistakes - only flag if script is JUST "return false" or "return true"
      // Complex scripts with conditionals should not trigger this
      const trimmedScript = script.trim().replace(/\s+/g, ' ');
      const isSimpleReturnFalse = /^return\s+false;?$/.test(trimmedScript);
      const isSimpleReturnTrue = /^return\s+true;?$/.test(trimmedScript);

      if (isSimpleReturnFalse) {
        warnings.push(
          `Agent '${agent.id}' logic is just 'return false'. Agent will never trigger.`
        );
      }

      if (isSimpleReturnTrue) {
        warnings.push(
          `Agent '${agent.id}' logic is just 'return true'. Consider adding conditions or removing the logic block.`
        );
      }

      // Check for undefined variable access (common typos)
      const knownVars = [
        'ledger',
        'cluster',
        'message',
        'agent',
        'helpers',
        'Set',
        'Map',
        'Array',
        'Object',
        'JSON',
        'Date',
        'Math',
      ];
      const varPattern = /\b([a-zA-Z_]\w*)\s*\./g;
      let match;
      while ((match = varPattern.exec(script)) !== null) {
        const varName = match[1];
        if (
          !knownVars.includes(varName) &&
          !script.includes(`const ${varName}`) &&
          !script.includes(`let ${varName}`)
        ) {
          warnings.push(
            `Agent '${agent.id}' logic uses '${varName}' which may be undefined. ` +
              `Available: [${knownVars.join(', ')}]`
          );
          break; // Only warn once per agent
        }
      }
    }
  }

  return { errors, warnings };
}

/**
 * Phase 5: Validate template variables against jsonSchema
 * Ensures {{result.*}} references in hooks match defined schema properties
 */
function validateTemplateVariables(config, depth = 0) {
  const errors = [];
  const warnings = [];

  if (!config.agents || !Array.isArray(config.agents)) {
    return { errors, warnings };
  }

  const prefix = depth > 0 ? `Sub-cluster (depth ${depth}): ` : '';

  for (const agent of config.agents) {
    // Skip subclusters - they have their own validation
    if (agent.type === 'subcluster') {
      // Recursively validate subcluster config
      if (agent.config?.agents) {
        const subResult = validateTemplateVariables(agent.config, depth + 1);
        // Prefix sub-cluster errors with agent ID
        errors.push(...subResult.errors.map((e) => `Sub-cluster '${agent.id}': ${e}`));
        warnings.push(...subResult.warnings.map((w) => `Sub-cluster '${agent.id}': ${w}`));
      }
      continue;
    }

    const result = validateAgentTemplateVariables(agent, agent.id);
    errors.push(...result.errors.map((e) => `${prefix}${e}`));
    warnings.push(...result.warnings.map((w) => `${prefix}${w}`));
  }

  return { errors, warnings };
}

/**
 * Validate template variables for a single agent
 * @param {Object} agent - Agent configuration
 * @param {String} agentId - Agent ID for error messages
 * @returns {{ errors: string[], warnings: string[] }}
 */
function validateAgentTemplateVariables(agent, agentId) {
  const errors = [];
  const warnings = [];

  // Extract schema properties (null if non-JSON output or text output)
  const schemaProps = extractSchemaProperties(agent);

  // If schemaProps is null, this agent doesn't use JSON output - skip validation
  if (schemaProps === null) {
    return { errors, warnings };
  }

  // Extract template variables from hooks
  const templateVars = extractTemplateVariables(agent);

  // Check for undefined references (ERROR)
  for (const varName of templateVars) {
    if (!schemaProps.has(varName)) {
      const availableProps = Array.from(schemaProps).join(', ');
      errors.push(
        `Agent '${agentId}': Template uses '{{result.${varName}}}' but '${varName}' is not defined in jsonSchema. ` +
          `Available properties: [${availableProps}]`
      );
    }
  }

  // Check for unused schema properties (WARNING)
  for (const prop of schemaProps) {
    if (!templateVars.has(prop)) {
      warnings.push(
        `Agent '${agentId}': Schema property '${prop}' is defined but never referenced in hooks. ` +
          `Consider removing it to save tokens.`
      );
    }
  }

  return { errors, warnings };
}

/**
 * Extract all template variables ({{result.*}}) from agent hooks
 * Searches hooks.onComplete.config (recursive) and hooks.onComplete.transform.script
 * Also searches triggers[].onComplete patterns
 * @param {Object} agent - Agent configuration
 * @returns {Set<string>} Set of variable names referenced
 */
function extractTemplateVariables(agent) {
  const variables = new Set();

  // Regex patterns - reset lastIndex before each use to avoid state pollution
  const mustachePattern = /\{\{result\.([^}]+)\}\}/g;
  const directPattern = /\bresult\.([a-zA-Z_][a-zA-Z0-9_]*)/g;

  /**
   * Recursively traverse an object/array and extract template variables from strings
   */
  function traverseAndExtract(obj) {
    if (obj === null || obj === undefined) {
      return;
    }

    if (typeof obj === 'string') {
      // Extract mustache-style {{result.field}}
      mustachePattern.lastIndex = 0;
      let match;
      while ((match = mustachePattern.exec(obj)) !== null) {
        variables.add(match[1]);
      }
      return;
    }

    if (Array.isArray(obj)) {
      for (const item of obj) {
        traverseAndExtract(item);
      }
      return;
    }

    if (typeof obj === 'object') {
      for (const value of Object.values(obj)) {
        traverseAndExtract(value);
      }
    }
  }

  /**
   * Extract variables from transform script (direct result.field access)
   */
  function extractFromScript(script) {
    if (typeof script !== 'string') {
      return;
    }

    directPattern.lastIndex = 0;
    let match;
    while ((match = directPattern.exec(script)) !== null) {
      variables.add(match[1]);
    }
  }

  // Extract from hooks.onComplete.config
  if (agent.hooks?.onComplete?.config) {
    traverseAndExtract(agent.hooks.onComplete.config);
  }

  // Extract from hooks.onComplete.transform.script
  if (agent.hooks?.onComplete?.transform?.script) {
    extractFromScript(agent.hooks.onComplete.transform.script);
  }

  // Extract from triggers[].onComplete (some agents define hooks per-trigger)
  if (agent.triggers && Array.isArray(agent.triggers)) {
    for (const trigger of agent.triggers) {
      if (trigger.onComplete?.config) {
        traverseAndExtract(trigger.onComplete.config);
      }
      if (trigger.onComplete?.transform?.script) {
        extractFromScript(trigger.onComplete.transform.script);
      }
    }
  }

  return variables;
}

/**
 * Extract schema properties from agent's jsonSchema
 * @param {Object} agent - Agent configuration
 * @returns {Set<string>|null} Set of property names, or null if agent doesn't use JSON output
 */
function extractSchemaProperties(agent) {
  // Non-JSON agents don't need validation
  // Both 'json' and 'stream-json' use jsonSchema and need validation
  if (!['json', 'stream-json'].includes(agent.outputFormat)) {
    return null;
  }

  // If explicit schema is provided, use its properties
  if (agent.jsonSchema?.properties) {
    return new Set(Object.keys(agent.jsonSchema.properties));
  }

  // Default schema when outputFormat is 'json' but no explicit schema
  // See: agent-config.js:62-69
  return new Set(['summary', 'result']);
}

/**
 * Check if iteration pattern is valid
 */
function isValidIterationPattern(pattern) {
  if (pattern === 'all') return true;
  if (/^\d+$/.test(pattern)) return true; // "1"
  if (/^\d+-\d+$/.test(pattern)) return true; // "1-3"
  if (/^\d+\+$/.test(pattern)) return true; // "5+"
  return false;
}

/**
 * Format validation result for CLI output
 */
function formatValidationResult(result) {
  const lines = [];

  if (result.valid) {
    lines.push('✅ Configuration is valid');
  } else {
    lines.push('❌ Configuration has errors');
  }

  if (result.errors.length > 0) {
    lines.push('\nErrors:');
    for (const error of result.errors) {
      lines.push(`  ❌ ${error}`);
    }
  }

  if (result.warnings.length > 0) {
    lines.push('\nWarnings:');
    for (const warning of result.warnings) {
      lines.push(`  ⚠️  ${warning}`);
    }
  }

  return lines.join('\n');
}

module.exports = {
  validateConfig,
  isConductorConfig,
  validateBasicStructure,
  analyzeMessageFlow,
  validateAgents,
  validateLogicScripts,
  isValidIterationPattern,
  formatValidationResult,
  // Phase 5: Template variable validation
  validateTemplateVariables,
  extractTemplateVariables,
  extractSchemaProperties,
  validateAgentTemplateVariables,
};
