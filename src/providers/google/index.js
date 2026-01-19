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

class GoogleProvider extends BaseProvider {
  constructor() {
    super({ name: 'gemini', displayName: 'Gemini', cliCommand: 'gemini' });
    this._cliFeatures = null;
    this._unknownEventCounts = new Map();
    this._parserState = { lastToolId: null };
  }

  // SDK not implemented - uses CLI only
  // See BaseProvider for SDK extension point documentation

  isAvailable() {
    return commandExists(this.cliCommand);
  }

  getCliPath() {
    return getCommandPath(this.cliCommand) || this.cliCommand;
  }

  getInstallInstructions() {
    return 'npm install -g @google/gemini-cli';
  }

  getAuthInstructions() {
    return 'gemini auth login';
  }

  getCliFeatures() {
    if (this._cliFeatures) return this._cliFeatures;
    const help = getHelpOutput(this.cliCommand);
    const unknown = !help;

    const features = {
      supportsStreamJson: unknown ? true : /--output-format\b/.test(help),
      supportsAutoApprove: unknown ? true : /--yolo\b/.test(help),
      supportsCwd: unknown ? true : /--cwd\b/.test(help),
      supportsModel: unknown ? true : /\s-m\b/.test(help) || /--model\b/.test(help),
      unknown,
    };

    this._cliFeatures = features;
    return features;
  }

  getCredentialPaths() {
    return ['~/.config/gcloud', '~/.config/gemini', '~/.gemini'];
  }

  buildCommand(context, options) {
    const cliFeatures = options.cliFeatures || {};

    if (options.autoApprove && cliFeatures.supportsAutoApprove === false) {
      this._warnOnce(
        'gemini-auto-approve',
        'Gemini CLI does not support --yolo; continuing without auto-approve.'
      );
    }

    return buildCommand(context, { ...options, cliFeatures });
  }

  parseEvent(line) {
    return parseEvent(line, this._parserState, {
      onUnknown: (type) => this._logUnknown(type),
    });
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

  _logUnknown(type) {
    const current = this._unknownEventCounts.get(type) || 0;
    if (current >= 5) return;
    this._unknownEventCounts.set(type, current + 1);
    console.debug(`[gemini] Unknown event type: ${type}`);
  }
}

module.exports = GoogleProvider;
