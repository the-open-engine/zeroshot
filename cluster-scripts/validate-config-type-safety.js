#!/usr/bin/env node

/**
 * Validates cluster configs for type-safety bugs
 *
 * DETECTS:
 * - Boolean template substitution in JSON (becomes string)
 * - Trigger logic comparing with wrong type
 * - Missing dual type checks (bool || string)
 *
 * WHY: Template substitution like "{{result.approved}}" converts boolean
 * to STRING in JSON, causing trigger comparisons to fail silently.
 */

const fs = require('fs');
const path = require('path');
const _vm = require('vm');

const TEMPLATES_DIR = path.join(__dirname, '../cluster-templates');
const AGENT_LIBRARY = path.join(__dirname, '../agent-library.json');

// Patterns that indicate type mismatch bugs
const DANGEROUS_PATTERNS = [
  {
    name: 'Boolean comparison without string fallback',
    pattern: /===\s*(true|false)(?!.*\|\|.*===\s*['"`](true|false))/,
    severity: 'ERROR',
    message:
      'Comparing with boolean literal without string fallback. Template substitution creates strings!',
  },
  {
    name: 'Template substitution of boolean in data field',
    pattern: /"approved":\s*"{{[^}]+}}"/,
    severity: 'WARNING',
    message:
      'Boolean value in template substitution will become string. Ensure trigger logic handles both types.',
  },
];

// Safe patterns (properly handle both types)
const SAFE_PATTERNS = [
  /===\s*false\s*\|\|\s*.*===\s*['"`]false['"`]/, // approved === false || approved === 'false'
  /===\s*true\s*\|\|\s*.*===\s*['"`]true['"`]/, // approved === true || approved === 'true'
  /===\s*['"`]false['"`]\s*\|\|\s*.*===\s*false/, // approved === 'false' || approved === false
  /===\s*['"`]true['"`]\s*\|\|\s*.*===\s*true/, // approved === 'true' || approved === true
];

function isSafe(code) {
  return SAFE_PATTERNS.some((pattern) => pattern.test(code));
}

function validateFile(filePath) {
  const content = fs.readFileSync(filePath, 'utf8');
  const issues = [];

  try {
    const config = JSON.parse(content);

    // Check all agents in config
    // Handle both array (templates) and object (agent-library.json)
    let agents = config.agents || [];
    if (!Array.isArray(agents)) {
      // agent-library.json has agents as object with agent definitions
      agents = Object.values(agents);
    }
    agents.forEach((agent) => {
      // Check triggers with logic scripts
      (agent.triggers || []).forEach((trigger) => {
        if (trigger.logic?.script) {
          const script = trigger.logic.script;

          // Skip if already safe
          if (isSafe(script)) {
            return;
          }

          // Check for dangerous patterns
          DANGEROUS_PATTERNS.forEach((dangerous) => {
            if (dangerous.pattern.test(script)) {
              issues.push({
                file: path.relative(process.cwd(), filePath),
                agent: agent.id,
                trigger: trigger.topic,
                severity: dangerous.severity,
                pattern: dangerous.name,
                message: dangerous.message,
                code: script.substring(0, 100) + '...',
              });
            }
          });
        }
      });

      // Check hooks for template substitution type issues
      if (agent.hooks?.onComplete) {
        const hook = agent.hooks.onComplete;
        const hookStr = JSON.stringify(hook);

        DANGEROUS_PATTERNS.forEach((dangerous) => {
          if (dangerous.pattern.test(hookStr)) {
            // Only warn if there's also a trigger in THIS cluster that might fail
            const hasVulnerableTrigger = agents.some((a) =>
              (a.triggers || []).some(
                (t) =>
                  t.logic?.script &&
                  !isSafe(t.logic.script) &&
                  /===\s*(true|false)/.test(t.logic.script)
              )
            );

            if (hasVulnerableTrigger) {
              issues.push({
                file: path.relative(process.cwd(), filePath),
                agent: agent.id,
                hook: 'onComplete',
                severity: 'WARNING',
                pattern: dangerous.name,
                message:
                  'Boolean template substitution detected. Ensure consuming triggers handle string type.',
                code: hookStr.substring(0, 100) + '...',
              });
            }
          }
        });
      }
    });
  } catch (error) {
    issues.push({
      file: path.relative(process.cwd(), filePath),
      severity: 'ERROR',
      pattern: 'Parse error',
      message: error.message,
    });
  }

  return issues;
}

function scanDirectory(dir) {
  const allIssues = [];

  function scan(currentDir) {
    const entries = fs.readdirSync(currentDir, { withFileTypes: true });

    for (const entry of entries) {
      const fullPath = path.join(currentDir, entry.name);

      if (entry.isDirectory()) {
        scan(fullPath);
      } else if (entry.name.endsWith('.json') && !entry.name.includes('package')) {
        const issues = validateFile(fullPath);
        allIssues.push(...issues);
      }
    }
  }

  scan(dir);
  return allIssues;
}

function main() {
  console.log('🔍 Validating cluster configs for type-safety issues...\n');

  // Scan all templates
  const issues = scanDirectory(TEMPLATES_DIR);

  // Also check agent library
  if (fs.existsSync(AGENT_LIBRARY)) {
    issues.push(...validateFile(AGENT_LIBRARY));
  }

  if (issues.length === 0) {
    console.log('✅ No type-safety issues found!\n');
    process.exit(0);
  }

  // Group by severity
  const errors = issues.filter((i) => i.severity === 'ERROR');
  const warnings = issues.filter((i) => i.severity === 'WARNING');

  if (errors.length > 0) {
    console.log(`❌ ERRORS (${errors.length}):\n`);
    errors.forEach((issue) => {
      console.log(`  File: ${issue.file}`);
      if (issue.agent) console.log(`  Agent: ${issue.agent}`);
      if (issue.trigger) console.log(`  Trigger: ${issue.trigger}`);
      if (issue.hook) console.log(`  Hook: ${issue.hook}`);
      console.log(`  Issue: ${issue.pattern}`);
      console.log(`  ${issue.message}`);
      if (issue.code) console.log(`  Code: ${issue.code}`);
      console.log('');
    });
  }

  if (warnings.length > 0) {
    console.log(`⚠️  WARNINGS (${warnings.length}):\n`);
    warnings.forEach((issue) => {
      console.log(`  File: ${issue.file}`);
      if (issue.agent) console.log(`  Agent: ${issue.agent}`);
      if (issue.trigger) console.log(`  Trigger: ${issue.trigger}`);
      if (issue.hook) console.log(`  Hook: ${issue.hook}`);
      console.log(`  Issue: ${issue.pattern}`);
      console.log(`  ${issue.message}`);
      if (issue.code) console.log(`  Code: ${issue.code}`);
      console.log('');
    });
  }

  console.log('\n📖 FIX GUIDE:\n');
  console.log('For trigger comparisons:');
  console.log('  ❌ BAD:  approved === false');
  console.log('  ✅ GOOD: approved === false || approved === "false"\n');
  console.log('Why: Template substitution like "{{result.approved}}" converts');
  console.log('     boolean values to STRINGS in JSON data fields.\n');

  process.exit(errors.length > 0 ? 1 : 0);
}

if (require.main === module) {
  main();
}

module.exports = { validateFile, scanDirectory };
