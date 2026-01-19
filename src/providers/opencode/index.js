const BaseProvider = require('../base-provider');
const { commandExists, getCommandPath, getHelpOutput } = require('../../../lib/provider-detection');
const { buildCommand } = require('./cli-builder');
const { parseEvent } = require('./output-parser');
const {
  MODEL_CATALOG,
  LEVEL_MAPPING,
  DEFAULT_LEVEL,
  DEFAULT_MAX_LEVEL,
  DEFAULT_MIN_LEVEL,
} = require('./models');

const warned = new Set();

class OpencodeProvider extends BaseProvider {
  constructor() {
    super({ name: 'opencode', displayName: 'Opencode', cliCommand: 'opencode' });
    this._cliFeatures = null;
  }

  isAvailable() {
    return commandExists(this.cliCommand);
  }

  getCliPath() {
    return getCommandPath(this.cliCommand) || this.cliCommand;
  }

  getInstallInstructions() {
    return 'See https://opencode.ai for installation instructions.';
  }

  getAuthInstructions() {
    return 'opencode auth login';
  }

  getCliFeatures() {
    if (this._cliFeatures) return this._cliFeatures;
    const help = getHelpOutput(this.cliCommand, ['run']);
    const unknown = !help;

    const features = {
      supportsJson: unknown ? true : /--format\b/.test(help),
      supportsModel: unknown ? true : /--model\b/.test(help),
      supportsVariant: unknown ? true : /--variant\b/.test(help),
      supportsCwd: unknown ? false : /--cwd\b/.test(help),
      supportsAutoApprove: false,
      unknown,
    };

    this._cliFeatures = features;
    return features;
  }

  getCredentialPaths() {
    return ['~/.local/share/opencode'];
  }

  buildCommand(context, options) {
    const cliFeatures = options.cliFeatures || {};

    if (options.modelSpec?.reasoningEffort && cliFeatures.supportsVariant === false) {
      this._warnOnce(
        'opencode-variant',
        'Opencode CLI does not support --variant; skipping reasoningEffort.'
      );
    }

    return buildCommand(context, { ...options, cliFeatures });
  }

  parseEvent(line) {
    return parseEvent(line);
  }

  getModelCatalog() {
    return MODEL_CATALOG;
  }

  getLevelMapping() {
    return LEVEL_MAPPING;
  }

  getDefaultLevel() {
    return DEFAULT_LEVEL;
  }

  getDefaultMaxLevel() {
    return DEFAULT_MAX_LEVEL;
  }

  getDefaultMinLevel() {
    return DEFAULT_MIN_LEVEL;
  }

  _warnOnce(key, message) {
    if (warned.has(key)) return;
    warned.add(key);
    console.warn(`⚠️ ${message}`);
  }
}

module.exports = OpencodeProvider;
