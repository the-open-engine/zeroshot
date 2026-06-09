const { readRepoSettings } = require('../lib/repo-settings');

function hasOwn(value, key) {
  return Object.prototype.hasOwnProperty.call(value || {}, key);
}

function hasDefinedOwn(value, key) {
  return hasOwn(value, key) && value[key] !== undefined;
}

function getGateId(gate) {
  if (typeof gate.id === 'string' && gate.id.trim()) {
    return gate.id.trim();
  }

  if (typeof gate.name === 'string' && gate.name.trim()) {
    return gate.name.trim();
  }

  return null;
}

function normalizeStringGate(gate) {
  const id = gate.trim();
  return id ? { id } : null;
}

function setOptionalString(target, source, key) {
  if (typeof source[key] === 'string' && source[key].trim()) {
    target[key] = source[key].trim();
  }
}

function normalizeObjectGate(gate) {
  if (!gate || typeof gate !== 'object') {
    return null;
  }

  const id = getGateId(gate);
  if (!id) {
    return null;
  }

  const normalized = { id };
  setOptionalString(normalized, gate, 'scope');
  setOptionalString(normalized, gate, 'description');
  setOptionalString(normalized, gate, 'command');
  return normalized;
}

function normalizeRequiredQualityGates(value) {
  if (!Array.isArray(value)) {
    return [];
  }

  return value
    .map((gate) => {
      if (typeof gate === 'string') {
        return normalizeStringGate(gate);
      }
      return normalizeObjectGate(gate);
    })
    .filter(Boolean);
}

function getRequiredQualityGateSource(options, repoSettings) {
  if (hasDefinedOwn(options, 'requiredQualityGates')) {
    return options.requiredQualityGates;
  }

  if (
    options.ship &&
    typeof options.ship === 'object' &&
    hasDefinedOwn(options.ship, 'requiredQualityGates')
  ) {
    return options.ship.requiredQualityGates;
  }

  const settingsShip = repoSettings?.ship;
  if (
    settingsShip &&
    typeof settingsShip === 'object' &&
    hasOwn(settingsShip, 'requiredQualityGates')
  ) {
    return settingsShip.requiredQualityGates;
  }

  if (hasOwn(repoSettings, 'requiredQualityGates')) {
    return repoSettings.requiredQualityGates;
  }

  return [];
}

function resolveRequiredQualityGates(options = {}) {
  const repoSettingsResult = readRepoSettings(options.cwd || process.cwd());
  const repoSettings = repoSettingsResult.settings || {};
  const source = getRequiredQualityGateSource(options, repoSettings);
  return normalizeRequiredQualityGates(source);
}

function getClusterRequiredQualityGateSource(config, options) {
  if (hasDefinedOwn(options, 'requiredQualityGates')) {
    return options.requiredQualityGates;
  }

  if (
    options.ship &&
    typeof options.ship === 'object' &&
    hasDefinedOwn(options.ship, 'requiredQualityGates')
  ) {
    return options.ship.requiredQualityGates;
  }

  if (
    config?.ship &&
    typeof config.ship === 'object' &&
    hasDefinedOwn(config.ship, 'requiredQualityGates')
  ) {
    return config.ship.requiredQualityGates;
  }

  if (hasDefinedOwn(config, 'requiredQualityGates')) {
    return config.requiredQualityGates;
  }

  return undefined;
}

function resolveClusterRequiredQualityGates(config = {}, options = {}) {
  const configuredSource = getClusterRequiredQualityGateSource(config, options);
  if (configuredSource !== undefined) {
    return normalizeRequiredQualityGates(configuredSource);
  }

  return resolveRequiredQualityGates({ ...options, cwd: options.cwd || process.cwd() });
}

module.exports = {
  normalizeRequiredQualityGates,
  resolveRequiredQualityGates,
  resolveClusterRequiredQualityGates,
};
