#!/usr/bin/env node
/**
 * Validate all cluster templates for config errors
 * Run in CI to prevent broken templates from being merged
 *
 * Usage: node scripts/validate-templates.js
 * Exit codes: 0 = all valid, 1 = validation errors found
 */

const fs = require('fs');
const path = require('path');
const { validateConfig } = require('../src/config-validator');

const TEMPLATES_DIR = path.join(__dirname, '../cluster-templates');

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

function validateTemplate(filePath) {
  const relativePath = path.relative(process.cwd(), filePath);

  try {
    const content = fs.readFileSync(filePath, 'utf-8');
    const config = JSON.parse(content);

    // Skip non-cluster configs (like package.json)
    if (!config.agents && !config.name) {
      return { valid: true, skipped: true };
    }

    const result = validateConfig(config);

    if (!result.valid) {
      console.error(`\n❌ ${relativePath}`);
      for (const error of result.errors) {
        console.error(`   ERROR: ${error}`);
      }
    } else if (result.warnings.length > 0) {
      console.warn(`\n⚠️  ${relativePath}`);
      for (const warning of result.warnings) {
        console.warn(`   WARN: ${warning}`);
      }
    } else {
      console.log(`✓ ${relativePath}`);
    }

    return result;
  } catch (err) {
    console.error(`\n❌ ${relativePath}`);
    console.error(`   PARSE ERROR: ${err.message}`);
    return { valid: false, errors: [err.message], warnings: [] };
  }
}

function main() {
  console.log('Validating cluster templates...\n');

  const templateFiles = [...findJsonFiles(TEMPLATES_DIR)];

  let hasErrors = false;
  let validated = 0;
  let skipped = 0;

  for (const file of templateFiles) {
    const result = validateTemplate(file);
    if (result.skipped) {
      skipped++;
    } else {
      validated++;
      if (!result.valid) {
        hasErrors = true;
      }
    }
  }

  console.log(`\n${'='.repeat(60)}`);
  console.log(`Validated: ${validated} templates, Skipped: ${skipped} files`);

  if (hasErrors) {
    console.error('\n❌ VALIDATION FAILED - Fix errors above before merging\n');
    process.exit(1);
  } else {
    console.log('\n✓ All templates valid\n');
    process.exit(0);
  }
}

main();
