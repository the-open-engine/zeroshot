/**
 * First-Run Setup Wizard
 *
 * Interactive setup on first use:
 * - Welcome banner
 * - Max model ceiling selection (sonnet/opus/haiku)
 * - Auto-update preference
 * - Marks setup as complete
 */

const readline = require('readline');
const { loadSettings, saveSettings } = require('../../lib/settings');

/**
 * Print welcome banner
 */
function printWelcome() {
  console.log(`
╔═══════════════════════════════════════════════════════════════╗
║                                                               ║
║   Welcome to Zeroshot!                                        ║
║   Multi-agent orchestration for Claude                        ║
║                                                               ║
║   Let's configure a few settings to get started.              ║
║                                                               ║
╚═══════════════════════════════════════════════════════════════╝
`);
}

/**
 * Create readline interface
 * @returns {readline.Interface}
 */
function createReadline() {
  return readline.createInterface({
    input: process.stdin,
    output: process.stdout,
  });
}

/**
 * Prompt for model selection
 * @param {readline.Interface} rl
 * @returns {Promise<string>}
 */
function promptModel(rl) {
  return new Promise((resolve) => {
    console.log('What is the maximum model agents can use? (cost ceiling)\n');
    console.log('  1) sonnet  - Agents can use sonnet or haiku (recommended)');
    console.log('  2) opus    - Agents can use opus, sonnet, or haiku');
    console.log('  3) haiku   - Agents can only use haiku (lowest cost)\n');

    rl.question('Enter 1, 2, or 3 [1]: ', (answer) => {
      const choice = answer.trim() || '1';
      switch (choice) {
        case '2':
          resolve('opus');
          break;
        case '3':
          resolve('haiku');
          break;
        default:
          resolve('sonnet');
      }
    });
  });
}

/**
 * Prompt for auto-update preference
 * @param {readline.Interface} rl
 * @returns {Promise<boolean>}
 */
function promptAutoUpdate(rl) {
  return new Promise((resolve) => {
    console.log('\nWould you like zeroshot to check for updates automatically?');
    console.log('(Checks npm registry every 24 hours)\n');

    rl.question('Enable auto-update checks? [Y/n]: ', (answer) => {
      const normalized = answer.trim().toLowerCase();
      // Default to yes if empty or starts with 'y'
      resolve(normalized === '' || normalized === 'y' || normalized === 'yes');
    });
  });
}

/**
 * Print completion message
 * @param {object} settings - Saved settings
 */
function printComplete(settings) {
  console.log(`
╔═══════════════════════════════════════════════════════════════╗
║  Setup complete!                                              ║
╚═══════════════════════════════════════════════════════════════╝

Your settings:
  • Max model:    ${settings.maxModel} (agents can use this model or lower)
  • Auto-updates: ${settings.autoCheckUpdates ? 'enabled' : 'disabled'}

Change anytime with: zeroshot settings set <key> <value>

Get started:
  zeroshot run "Fix the bug in auth.js"
  zeroshot run 123  (GitHub issue number)
  zeroshot --help

`);
}

/**
 * Check if first-run setup is needed
 * @param {object} settings - Current settings
 * @returns {boolean}
 */
function detectFirstRun(settings) {
  return !settings.firstRunComplete;
}

/**
 * Main entry point - run first-time setup if needed
 * @param {object} options
 * @param {boolean} options.quiet - Skip interactive prompts
 * @returns {Promise<boolean>} True if setup was run
 */
async function checkFirstRun(options = {}) {
  const settings = loadSettings();

  // Already completed setup
  if (!detectFirstRun(settings)) {
    return false;
  }

  // Quiet mode - use defaults, mark complete
  if (options.quiet) {
    settings.firstRunComplete = true;
    saveSettings(settings);
    return true;
  }

  // Interactive setup
  printWelcome();

  const rl = createReadline();

  try {
    // Model ceiling selection
    const model = await promptModel(rl);
    settings.maxModel = model;

    // Auto-update preference
    const autoUpdate = await promptAutoUpdate(rl);
    settings.autoCheckUpdates = autoUpdate;

    // Mark complete
    settings.firstRunComplete = true;
    saveSettings(settings);

    // Print completion
    printComplete(settings);

    return true;
  } finally {
    rl.close();
  }
}

module.exports = {
  checkFirstRun,
  // Exported for testing
  detectFirstRun,
  printWelcome,
  printComplete,
};
