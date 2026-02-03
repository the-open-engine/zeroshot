#!/usr/bin/env node
/**
 * Validate all cluster templates for config errors
 * Run in CI to prevent broken templates from being merged
 *
 * Usage: node scripts/validate-templates.js
 * Exit codes: 0 = all valid, 1 = validation errors found
 */

const path = require('path');
const { validateTemplates } = require('../src/template-validation');

const TEMPLATES_DIR = path.join(__dirname, '../cluster-templates');

function parseArgs(argv) {
  const deep =
    argv.includes('--deep') ||
    argv.includes('--sim=deep') ||
    process.env.ZEROSHOT_TEMPLATE_SIM === 'deep';
  return { deep };
}

async function main() {
  console.log('Validating cluster templates...\n');

  const { deep } = parseArgs(process.argv.slice(2));
  const report = await validateTemplates({ templatesDir: TEMPLATES_DIR, deep });

  let hasErrors = false;

  for (const { filePath, result } of report.results) {
    const relativePath = path.relative(process.cwd(), filePath);

    if (!result.valid) {
      hasErrors = true;
      console.error(`\n❌ ${relativePath}`);
      for (const error of result.errors) {
        console.error(`   ERROR: ${error}`);
      }
      continue;
    }

    if (result.warnings.length > 0) {
      console.warn(`\n⚠️  ${relativePath}`);
      for (const warning of result.warnings) {
        console.warn(`   WARN: ${warning}`);
      }
      continue;
    }

    console.log(`✓ ${relativePath}`);
  }

  console.log(`\n${'='.repeat(60)}`);
  console.log(`Validated: ${report.validated} templates, Skipped: ${report.skipped} files`);

  if (hasErrors) {
    console.error('\n❌ VALIDATION FAILED - Fix errors above before merging\n');
    process.exit(1);
  } else {
    console.log('\n✓ All templates valid\n');
    process.exit(0);
  }
}

main().catch((err) => {
  console.error(`\n❌ Template validation crashed: ${err.message}\n`);
  process.exit(1);
});
