const fs = require('node:fs');
const path = require('node:path');

const { validateConfig } = require('../config-validator');
const { getConfig } = require('../config-router');
const TemplateResolver = require('../template-resolver');
const { SHARED_TRIGGER_SCRIPT } = require('../agents/git-pusher-template');
const { simulateConsensusGates } = require('./simulate-consensus-gates');
const { simulateTwoStageValidation } = require('./simulate-two-stage-validation');

const CONDUCTOR_COMPLEXITIES = ['TRIVIAL', 'SIMPLE', 'STANDARD', 'CRITICAL'];
const CONDUCTOR_TASK_TYPES = ['TASK', 'DEBUG', 'INQUIRY'];

function findJsonFiles(dir) {
  const files = [];
  if (!fs.existsSync(dir)) return files;

  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const fullPath = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      files.push(...findJsonFiles(fullPath));
    } else if (entry.name.endsWith('.json')) {
      files.push(fullPath);
    }
  }
  return files;
}

function inferTemplateIdFromPath(filePath) {
  const base = path.basename(filePath, '.json');
  return base || 'unknown';
}

async function validateTemplateConfig({ config, templateId, deep }) {
  const result = validateConfig(config);

  if (result.valid) {
    const simErrors = [];
    simErrors.push(...simulateConsensusGates(config));
    if (deep) {
      simErrors.push(...(await simulateTwoStageValidation({ templateId, config })));
    }
    if (simErrors.length > 0) {
      result.valid = false;
      result.errors.push(...simErrors);
    }
  }

  return result;
}

function hasCompletionHandler(config) {
  const agents = Array.isArray(config?.agents) ? config.agents : [];
  return agents.some(
    (agent) =>
      agent.id === 'completion-detector' ||
      agent.id === 'git-pusher' ||
      agent.hooks?.onComplete?.config?.topic === 'CLUSTER_COMPLETE' ||
      agent.triggers?.some((trigger) => trigger.action === 'stop_cluster')
  );
}

function mergeAgentsById(primaryConfig, additionalConfig) {
  const merged = {
    ...primaryConfig,
    agents: Array.isArray(primaryConfig?.agents) ? [...primaryConfig.agents] : [],
  };
  const seen = new Set(merged.agents.map((agent) => agent.id));

  for (const agent of additionalConfig?.agents || []) {
    if (seen.has(agent.id)) {
      continue;
    }
    merged.agents.push(agent);
    seen.add(agent.id);
  }

  return merged;
}

function augmentCriticalResolvedConfig({ resolver, config, params }) {
  if (params?.complexity !== 'CRITICAL' || params?.validator_count !== 0) {
    return config;
  }

  const quickValidationConfig = resolver.resolve('quick-validation', {
    validator_level: params.validator_level,
    max_tokens: params.max_tokens,
    timeout: params.timeout || 0,
  });
  return mergeAgentsById(config, quickValidationConfig);
}

function ensureCompletionHandler(config) {
  if (hasCompletionHandler(config)) {
    return config;
  }

  return {
    ...config,
    agents: [
      ...(config.agents || []),
      {
        id: 'completion-detector',
        role: 'orchestrator',
        modelLevel: 'level1',
        triggers: [
          {
            topic: 'VALIDATION_RESULT',
            logic: {
              engine: 'javascript',
              script: SHARED_TRIGGER_SCRIPT,
            },
            action: 'stop_cluster',
          },
        ],
      },
    ],
  };
}

function buildResolvedConductorRoutes(templatesDir) {
  const conductorTemplatePath = path.join(templatesDir, 'conductor-bootstrap.json');
  if (!fs.existsSync(conductorTemplatePath)) {
    return [];
  }

  const resolver = new TemplateResolver(templatesDir);
  const routeConfigs = [];

  for (const complexity of CONDUCTOR_COMPLEXITIES) {
    for (const taskType of CONDUCTOR_TASK_TYPES) {
      const { base, params } = getConfig(complexity, taskType);
      const resolved = resolver.resolve(base, params);
      const augmented = augmentCriticalResolvedConfig({
        resolver,
        config: resolved,
        params,
      });
      const configWithCompletion = ensureCompletionHandler(augmented);

      routeConfigs.push({
        filePath: `${conductorTemplatePath}#resolved:${complexity}-${taskType}`,
        templateId: `resolved-${base}`,
        config: configWithCompletion,
      });
    }
  }

  return routeConfigs;
}

async function validateTemplates({ templatesDir, deep = false }) {
  const templateFiles = [...findJsonFiles(templatesDir)];
  const resolvedConductorRoutes = buildResolvedConductorRoutes(templatesDir);

  let hasErrors = false;
  let validated = 0;
  let skipped = 0;
  const results = [];

  for (const filePath of templateFiles) {
    try {
      const content = fs.readFileSync(filePath, 'utf-8');
      const config = JSON.parse(content);

      // Skip non-cluster configs (like package.json)
      if (!config.agents && !config.name) {
        skipped++;
        continue;
      }

      const templateId = inferTemplateIdFromPath(filePath);
      const result = await validateTemplateConfig({ config, templateId, deep });

      results.push({ filePath, result });
      validated++;
      if (!result.valid) hasErrors = true;
    } catch (err) {
      results.push({ filePath, result: { valid: false, errors: [err.message], warnings: [] } });
      validated++;
      hasErrors = true;
    }
  }

  for (const resolvedRoute of resolvedConductorRoutes) {
    try {
      const result = await validateTemplateConfig({
        config: resolvedRoute.config,
        templateId: resolvedRoute.templateId,
        deep,
      });
      results.push({ filePath: resolvedRoute.filePath, result });
      validated++;
      if (!result.valid) hasErrors = true;
    } catch (err) {
      results.push({
        filePath: resolvedRoute.filePath,
        result: { valid: false, errors: [err.message], warnings: [] },
      });
      validated++;
      hasErrors = true;
    }
  }

  return {
    valid: !hasErrors,
    validated,
    skipped,
    results,
  };
}

module.exports = {
  validateTemplates,
  validateTemplateConfig,
};
