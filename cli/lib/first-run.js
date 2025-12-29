/**
 * First-Run Setup Wizard
 *
 * Interactive setup on first use:
 * - Welcome banner
 * - Default model selection (sonnet/opus/haiku)
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
    console.log('Which Claude model should agents use by default?\n');
    console.log('  1) sonnet  - Fast & capable (recommended)');
    console.log('  2) opus    - Most capable, slower');
    console.log('  3) haiku   - Fastest, for simple tasks\n');

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
  • Default model: ${settings.defaultModel}
  • Auto-updates:  ${settings.autoCheckUpdates ? 'enabled' : 'disabled'}

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
    // Model selection
    const model = await promptModel(rl);
    settings.defaultModel = model;

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
