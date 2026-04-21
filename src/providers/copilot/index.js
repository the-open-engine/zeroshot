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

class CopilotProvider extends BaseProvider {
  constructor() {
    super({ name: 'copilot', displayName: 'Copilot', cliCommand: 'copilot' });
    this._cliFeatures = null;
  }

  isAvailable() {
    return commandExists(this.cliCommand);
  }

  getCliPath() {
    return getCommandPath(this.cliCommand) || this.cliCommand;
  }

  getInstallInstructions() {
    return 'npm install -g @github/copilot';
  }

  getAuthInstructions() {
    return 'Run `copilot` then type `/login` (requires a GitHub account with Copilot access).';
  }

  getCliFeatures() {
    if (this._cliFeatures) return this._cliFeatures;
    const help = getHelpOutput(this.cliCommand, []);
    const unknown = !help;

    const features = {
      supportsModel: unknown ? true : /--model\b/.test(help),
      supportsAllowAll: unknown ? true : /--allow-all\b|--yolo\b/.test(help),
      supportsSilent: unknown ? true : /--silent\b/.test(help),
      supportsNoCustomInstructions: unknown ? true : /--no-custom-instructions\b/.test(help),
      supportsAutoApprove: unknown ? true : /--allow-all\b|--yolo\b/.test(help),
      unknown,
    };

    this._cliFeatures = features;
    return features;
  }

  getCredentialPaths() {
    return ['~/.copilot'];
  }

  buildCommand(context, options) {
    const cliFeatures = options.cliFeatures || this.getCliFeatures();

    if (options.modelSpec?.model && cliFeatures.supportsModel === false) {
      this._warnOnce(
        'copilot-model',
        'Copilot CLI help did not advertise --model; passing it anyway may fail.'
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

module.exports = CopilotProvider;
