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

class NovitaProvider extends BaseProvider {
  constructor() {
    super({ name: 'novita', displayName: 'Novita', cliCommand: 'codex' });
    this._cliFeatures = null;
    this._unknownEventCounts = new Map();
  }

  getRetryableErrorPatterns() {
    return [
      ...super.getRetryableErrorPatterns(),
      /rate_limit_exceeded/i,
      /\bserver_error\b/i,
      /\bservice_unavailable\b/i,
    ];
  }

  getPermanentErrorPatterns() {
    return [
      ...super.getPermanentErrorPatterns(),
      /invalid_api_key/i,
      /\binsufficient_quota\b/i,
      /\bmodel_not_found\b/i,
      /\bcontext_length_exceeded\b/i,
    ];
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
    return 'npm install -g @openai/codex';
  }

  getAuthInstructions() {
    return 'codex login (configure NOVITA_API_KEY environment variable)';
  }

  getCliFeatures() {
    if (this._cliFeatures) return this._cliFeatures;
    // Check 'codex exec --help' not 'codex --help'
    const help = getHelpOutput(this.cliCommand, ['exec']);
    const unknown = !help;

    const features = {
      supportsJson: unknown ? true : /--json\b/.test(help),
      supportsOutputSchema: unknown ? true : /--output-schema\b/.test(help),
      supportsAutoApprove: unknown
        ? true
        : /--dangerously-bypass-approvals-and-sandbox\b/.test(help),
      supportsCwd: unknown ? true : /\s-C\b/.test(help) || /--cwd\b/.test(help),
      supportsConfigOverride: unknown ? true : /--config\b/.test(help),
      supportsModel: unknown ? true : /\s-m\b/.test(help) || /--model\b/.test(help),
      supportsSkipGitRepoCheck: unknown ? true : /--skip-git-repo-check\b/.test(help),
      unknown,
    };

    this._cliFeatures = features;
    return features;
  }

  getCredentialPaths() {
    return ['~/.config/codex', '~/.codex'];
  }

  resolveAuthEnv() {
    const env = {};
    if (process.env.NOVITA_API_KEY) {
      env.OPENAI_API_KEY = process.env.NOVITA_API_KEY;
    }
    if (process.env.OPENAI_API_BASE || process.env.OPENAI_BASE_URL) {
      env.OPENAI_BASE_URL = process.env.OPENAI_API_BASE || process.env.OPENAI_BASE_URL;
    } else {
      env.OPENAI_BASE_URL = 'https://api.novita.ai/openai/v1';
    }
    return env;
  }

  buildCommand(context, options) {
    const cliFeatures = options.cliFeatures || {};

    if (options.autoApprove && cliFeatures.supportsAutoApprove === false) {
      this._warnOnce(
        'novita-auto-approve',
        'Codex CLI does not support auto-approve; continuing without bypass flag.'
      );
    }

    if (options.jsonSchema && cliFeatures.supportsOutputSchema === false) {
      this._warnOnce(
        'novita-jsonschema',
        'Codex CLI does not support --output-schema; skipping schema flag.'
      );
    }

    if (options.modelSpec?.reasoningEffort && cliFeatures.supportsConfigOverride === false) {
      this._warnOnce(
        'novita-reasoning',
        'Codex CLI does not support --config overrides; skipping reasoningEffort.'
      );
    }

    return buildCommand(context, { ...options, cliFeatures, authEnv: this.resolveAuthEnv() });
  }

  parseEvent(line) {
    return parseEvent(line, {
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
    console.debug(`[novita] Unknown event type: ${type}`);
  }
}

module.exports = NovitaProvider;
