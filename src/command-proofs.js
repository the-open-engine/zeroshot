const { readRepoSettings } = require('../lib/repo-settings');

const PROOF_BLOCK_PATTERN = /^```zeroshot-command-proofs\s*\n([\s\S]*?)^```/m;

function hasOwn(value, key) {
  return Object.prototype.hasOwnProperty.call(value || {}, key);
}

function trimString(value) {
  return typeof value === 'string' && value.trim() ? value.trim() : null;
}

function setOptionalString(target, source, key) {
  const value = trimString(source[key]);
  if (value) {
    target[key] = value;
  }
}

function normalizeCommandProof(proof) {
  if (!proof || typeof proof !== 'object') {
    return null;
  }

  const id = trimString(proof.id || proof.name);
  const profile = trimString(proof.profile || proof.proofProfile);
  const command = trimString(proof.command);
  if (!id || !profile || !command) {
    return null;
  }

  const normalized = { id, profile, command };
  setOptionalString(normalized, proof, 'scope');
  setOptionalString(normalized, proof, 'description');
  return normalized;
}

function normalizeCommandProofs(value) {
  if (!Array.isArray(value)) {
    return [];
  }
  return value.map(normalizeCommandProof).filter(Boolean);
}

function parseCommandProofsFromText(text) {
  if (typeof text !== 'string' || text.trim() === '') {
    return [];
  }

  const match = text.match(PROOF_BLOCK_PATTERN);
  if (!match) {
    return [];
  }

  let parsed;
  try {
    parsed = JSON.parse(match[1]);
  } catch (error) {
    throw new Error(`Invalid zeroshot-command-proofs JSON: ${error.message}`);
  }

  return normalizeCommandProofs(parsed);
}

function mergeCommandProofs(...sources) {
  const byId = new Map();
  const order = [];

  for (const source of sources) {
    for (const proof of normalizeCommandProofs(source)) {
      if (!byId.has(proof.id)) {
        order.push(proof.id);
      }
      byId.set(proof.id, proof);
    }
  }

  return order.map((id) => byId.get(id));
}

function commandProofToQualityGate(proof) {
  const normalized = normalizeCommandProof(proof);
  if (!normalized) {
    return null;
  }

  const gate = {
    id: normalized.id,
    profile: normalized.profile,
    command: normalized.command,
    commandProof: true,
  };
  setOptionalString(gate, normalized, 'scope');
  setOptionalString(gate, normalized, 'description');
  return gate;
}

function getCommandProofSource(options, repoSettings) {
  if (hasOwn(options, 'commandProofs')) {
    return options.commandProofs;
  }

  if (options.ship && typeof options.ship === 'object' && hasOwn(options.ship, 'commandProofs')) {
    return options.ship.commandProofs;
  }

  const settingsShip = repoSettings?.ship;
  if (settingsShip && typeof settingsShip === 'object' && hasOwn(settingsShip, 'commandProofs')) {
    return settingsShip.commandProofs;
  }

  if (hasOwn(repoSettings, 'commandProofs')) {
    return repoSettings.commandProofs;
  }

  return [];
}

function getClusterCommandProofSource(config, options) {
  if (hasOwn(options, 'commandProofs')) {
    return options.commandProofs;
  }

  if (options.ship && typeof options.ship === 'object' && hasOwn(options.ship, 'commandProofs')) {
    return options.ship.commandProofs;
  }

  if (config?.ship && typeof config.ship === 'object' && hasOwn(config.ship, 'commandProofs')) {
    return config.ship.commandProofs;
  }

  if (hasOwn(config, 'commandProofs')) {
    return config.commandProofs;
  }

  return undefined;
}

function resolveConfiguredCommandProofs(config = {}, options = {}) {
  const configuredSource = getClusterCommandProofSource(config, options);
  if (configuredSource !== undefined) {
    return normalizeCommandProofs(configuredSource);
  }

  const repoSettingsResult = readRepoSettings(options.cwd || process.cwd());
  const repoSettings = repoSettingsResult.settings || {};
  return normalizeCommandProofs(getCommandProofSource(options, repoSettings));
}

function resolveClusterCommandProofs(config = {}, options = {}, inputText = '') {
  return mergeCommandProofs(
    resolveConfiguredCommandProofs(config, options),
    parseCommandProofsFromText(inputText)
  );
}

function commandProofsToQualityGates(commandProofs) {
  return normalizeCommandProofs(commandProofs).map(commandProofToQualityGate).filter(Boolean);
}

module.exports = {
  commandProofToQualityGate,
  commandProofsToQualityGates,
  mergeCommandProofs,
  normalizeCommandProofs,
  parseCommandProofsFromText,
  resolveClusterCommandProofs,
  resolveConfiguredCommandProofs,
};
