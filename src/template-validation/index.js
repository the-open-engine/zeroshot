const fs = require('node:fs');
const path = require('node:path');

const { validateConfig } = require('../config-validator');
const { simulateConsensusGates } = require('./simulate-consensus-gates');
const { simulateTwoStageValidation } = require('./simulate-two-stage-validation');

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

async function validateTemplates({ templatesDir, deep = false }) {
  const templateFiles = [...findJsonFiles(templatesDir)];

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
