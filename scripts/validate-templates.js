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
  const simModeArg = argv.find((arg) => arg.startsWith('--sim='));
  const simMode = simModeArg ? simModeArg.split('=')[1] : process.env.ZEROSHOT_TEMPLATE_SIM;
  const randomScopeArg = argv.find((arg) => arg.startsWith('--random-scope='));
  const randomScope = randomScopeArg
    ? randomScopeArg.split('=')[1]
    : process.env.ZEROSHOT_TEMPLATE_RANDOM_SCOPE || 'resolved';
  const samplesArg = argv.find((arg) => arg.startsWith('--samples='));
  const sampleStepsArg = argv.find((arg) => arg.startsWith('--sample-steps='));
  const sampleMsArg = argv.find((arg) => arg.startsWith('--sample-ms='));

  const sampleCount = Number(
    samplesArg ? samplesArg.split('=')[1] : process.env.ZEROSHOT_TEMPLATE_SIM_SAMPLES
  );
  const sampleSteps = Number(
    sampleStepsArg ? sampleStepsArg.split('=')[1] : process.env.ZEROSHOT_TEMPLATE_SIM_STEPS
  );
  const sampleMs = Number(
    sampleMsArg ? sampleMsArg.split('=')[1] : process.env.ZEROSHOT_TEMPLATE_SIM_MS
  );

  const deep =
    argv.includes('--deep') ||
    argv.includes('--sim=deep') ||
    simMode === 'deep' ||
    simMode === 'all';
  const randomSampling = argv.includes('--sim=random') || simMode === 'random' || simMode === 'all';

  return {
    deep,
    randomSampling,
    randomScope,
    randomOptions: {
      samples: Number.isFinite(sampleCount) && sampleCount > 0 ? sampleCount : undefined,
      maxSteps: Number.isFinite(sampleSteps) && sampleSteps > 0 ? sampleSteps : undefined,
      maxScenarioMs: Number.isFinite(sampleMs) && sampleMs > 0 ? sampleMs : undefined,
    },
  };
}

async function main() {
  console.log('Validating cluster templates...\n');

  const { deep, randomSampling, randomScope, randomOptions } = parseArgs(process.argv.slice(2));
  const report = await validateTemplates({
    templatesDir: TEMPLATES_DIR,
    deep,
    randomSampling,
    randomScope,
    randomOptions,
  });

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
