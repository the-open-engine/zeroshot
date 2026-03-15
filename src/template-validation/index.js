const fs = require('node:fs');
const path = require('node:path');

const { validateConfig } = require('../config-validator');
const { getConfig } = require('../config-router');
const TemplateResolver = require('../template-resolver');
const { SHARED_TRIGGER_SCRIPT } = require('../agents/git-pusher-template');
const { simulateConsensusGates } = require('./simulate-consensus-gates');
const { simulateTwoStageValidation } = require('./simulate-two-stage-validation');
const { simulateRandomTopology } = require('./simulate-random-topology');

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

async function validateTemplateConfig({
  config,
  templateId,
  deep,
  randomSampling = false,
  templatesDir,
  randomOptions = {},
}) {
  const result = validateConfig(config);
  const isParameterizedTemplate = !!(config?.params && Object.keys(config.params).length > 0);

  // Composition templates lack ISSUE_OPENED/completion by design (get injected at runtime)
  const isCompositionTemplate = ['quick-validation', 'heavy-validation', 'full-workflow', 'worker-validator'].includes(templateId);

  if (result.valid || (deep && isCompositionTemplate)) {
    const simErrors = [];

    // Skip consensus gates for composition templates (incomplete by design)
    if (result.valid) {
      simErrors.push(
        ...simulateConsensusGates(config, {
          allowExternalTopics: isParameterizedTemplate
            ? ['IMPLEMENTATION_READY', 'QUICK_VALIDATION_PASSED']
            : [],
        })
      );
    }

    if (deep) {
      simErrors.push(...(await simulateTwoStageValidation({ templateId, config })));
    }
    if (randomSampling) {
      simErrors.push(
        ...(await simulateRandomTopology({
          config,
          templateId,
          templatesDir,
          ...randomOptions,
        }))
      );
    }

    if (simErrors.length > 0) {
      result.valid = false;
      result.errors.push(...simErrors);
    } else if (isCompositionTemplate && deep) {
      // Deep simulation passed - mark composition template as valid
      result.valid = true;
      result.errors = [];
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

function buildSyntheticGitPusher() {
  return {
    id: 'git-pusher',
    role: 'completion-detector',
    modelLevel: 'level1',
    triggers: [
      {
        topic: 'VALIDATION_RESULT',
        logic: {
          engine: 'javascript',
          script: SHARED_TRIGGER_SCRIPT,
        },
        action: 'execute_task',
      },
    ],
    hooks: {
      onComplete: {
        action: 'publish_message',
        config: {
          topic: 'CLUSTER_COMPLETE',
          content: {
            text: 'PR flow completed.',
          },
        },
      },
    },
  };
}

function buildSyntheticCompletionDetector() {
  return {
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
  };
}

function buildResolvedRoute({ conductorTemplatePath, resolver, complexity, taskType, autoPr }) {
  const { base, params } = getConfig(complexity, taskType, { autoPr });
  const resolved = resolver.resolve(base, params);
  const augmented = augmentCriticalResolvedConfig({
    resolver,
    config: resolved,
    params,
  });
  const configWithCompletion = ensureCompletionHandler(augmented, { autoPr });
  const modeSuffix = autoPr ? '-autopr' : '';

  return {
    filePath: `${conductorTemplatePath}#resolved${modeSuffix}:${complexity}-${taskType}`,
    templateId: `resolved-${base}${modeSuffix}`,
    config: configWithCompletion,
  };
}

function getRouteModes(taskType) {
  if (taskType === 'INQUIRY') {
    return [false];
  }

  return [false, true];
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
      for (const autoPr of getRouteModes(taskType)) {
        routeConfigs.push(
          buildResolvedRoute({
            conductorTemplatePath,
            resolver,
            complexity,
            taskType,
            autoPr,
          })
        );
      }
    }
  }

  return routeConfigs;
}

function shouldSkipConfig(config) {
  return !config.agents && !config.name;
}

function updateValidationSummary(summary, result) {
  summary.results.push(result);
  summary.validated++;
  if (!result.result.valid) {
    summary.hasErrors = true;
  }
}

async function validateConfigEntry({
  filePath,
  config,
  templateId,
  deep,
  randomSampling,
  templatesDir,
  randomOptions,
}) {
  try {
    const result = await validateTemplateConfig({
      config,
      templateId,
      deep,
      randomSampling,
      templatesDir,
      randomOptions,
    });

    return { filePath, result };
  } catch (err) {
    return {
      filePath,
      result: { valid: false, errors: [err.message], warnings: [] },
    };
  }
}

async function validateTemplates({
  templatesDir,
  deep = false,
  randomSampling = false,
  randomScope = 'resolved',
  randomOptions = {},
}) {
  const templateFiles = [...findJsonFiles(templatesDir)];
  const resolvedConductorRoutes = buildResolvedConductorRoutes(templatesDir);

  const summary = {
    hasErrors: false,
    validated: 0,
    skipped: 0,
    results: [],
  };

  // Resolver expects parent directory (e.g., cluster-templates) not base-templates
  const isBaseTemplatesDir = path.basename(templatesDir) === 'base-templates';
  const resolverDir = isBaseTemplatesDir ? path.dirname(templatesDir) : templatesDir;
  const resolver = new TemplateResolver(resolverDir);

  for (const filePath of templateFiles) {
    const content = fs.readFileSync(filePath, 'utf-8');
    const config = JSON.parse(content);

    if (shouldSkipConfig(config)) {
      summary.skipped++;
      continue;
    }

    const templateId = inferTemplateIdFromPath(filePath);

    // For deep validation, resolve parameterized templates with default values
    let configToValidate = config;
    if (deep && config.params && Object.keys(config.params).length > 0) {
      const defaultParams = {};
      for (const [key, paramDef] of Object.entries(config.params)) {
        // Use param default if available, otherwise pick first enum value for required params
        if (paramDef.default !== undefined) {
          defaultParams[key] = paramDef.default;
        } else if (paramDef.enum && paramDef.enum.length > 0) {
          defaultParams[key] = paramDef.enum[0];
        }
      }
      try {
        configToValidate = resolver.resolve(templateId, defaultParams);
      } catch (resolutionError) {
        // Resolution failed - likely missing required params without defaults
        // Skip deep validation for this template
        console.debug(
          `Skipping deep validation for ${templateId}: ${resolutionError.message}`
        );
        summary.skipped++;
        continue;
      }
    }

    const result = await validateConfigEntry({
      filePath,
      config: configToValidate,
      templateId,
      deep,
      randomSampling: randomSampling && randomScope === 'all',
      templatesDir,
      randomOptions,
    });
    updateValidationSummary(summary, result);
  }

  for (const resolvedRoute of resolvedConductorRoutes) {
    const result = await validateConfigEntry({
      filePath: resolvedRoute.filePath,
      config: resolvedRoute.config,
      templateId: resolvedRoute.templateId,
      deep,
      randomSampling,
      templatesDir,
      randomOptions,
    });
    updateValidationSummary(summary, result);
  }

  return {
    valid: !summary.hasErrors,
    validated: summary.validated,
    skipped: summary.skipped,
    results: summary.results,
  };
}

function ensureCompletionHandler(config, options = {}) {
  const { autoPr = false } = options;

  if (hasCompletionHandler(config)) {
    return config;
  }

  return {
    ...config,
    agents: [
      ...(config.agents || []),
      autoPr ? buildSyntheticGitPusher() : buildSyntheticCompletionDetector(),
    ],
  };
}

module.exports = {
  validateTemplates,
  validateTemplateConfig,
};
