function buildQualityGateSchema() {
  return {
    type: 'array',
    description: 'Tool-neutral ship handoff quality gate evidence for configured required gates.',
    items: {
      type: 'object',
      properties: {
        id: {
          type: 'string',
          description: 'Stable gate id matching the configured required quality gate.',
        },
        name: {
          type: 'string',
          description: 'Optional display name for the gate.',
        },
        status: {
          type: 'string',
          enum: ['PASS', 'FAIL', 'UNAVAILABLE'],
          description: 'PASS only when the gate completed successfully.',
        },
        scope: {
          type: 'string',
          description: 'Optional repo-defined scope for the gate.',
        },
        evidence: {
          type: 'object',
          properties: {
            command: { type: 'string' },
            exitCode: { type: 'integer' },
            output: { type: 'string' },
            proof: {
              type: 'object',
              description: 'Optional command proof metadata when the gate used cmdproof.',
              properties: {
                profile: { type: 'string' },
                reused: { type: 'boolean' },
                status: { type: 'string' },
              },
            },
          },
          required: ['command', 'exitCode', 'output'],
        },
        completedAt: {
          type: ['string', 'number'],
          description: 'When the gate completed, as an ISO string or numeric timestamp.',
        },
        timestamp: {
          type: ['string', 'number'],
          description: 'Alternate completion timestamp, as an ISO string or numeric timestamp.',
        },
        stale: {
          type: 'boolean',
          description: 'True when this evidence is known stale and must block handoff.',
        },
        reason: { type: 'string' },
      },
      required: ['id', 'status', 'evidence'],
    },
  };
}

function hasRequiredQualityGates(config) {
  return Array.isArray(config.requiredQualityGates) && config.requiredQualityGates.length > 0;
}

function shouldApplyValidatorQualityGateDefaults(config) {
  return (
    config.role === 'validator' && config.outputFormat === 'json' && hasRequiredQualityGates(config)
  );
}

function applyValidatorQualityGateSchema(config) {
  if (!shouldApplyValidatorQualityGateDefaults(config)) {
    return;
  }

  const properties = config.jsonSchema?.properties;
  if (properties && !properties.qualityGates) {
    properties.qualityGates = buildQualityGateSchema();
  }
}

function applyValidatorQualityGatePublishMapping(config) {
  if (!shouldApplyValidatorQualityGateDefaults(config)) {
    return;
  }

  const data = config.hooks?.onComplete?.config?.content?.data;
  if (data && data.qualityGates === undefined) {
    data.qualityGates = '{{result.qualityGates}}';
  }
}

function applyValidatorQualityGateDefaults(config) {
  applyValidatorQualityGateSchema(config);
  applyValidatorQualityGatePublishMapping(config);
}

module.exports = {
  applyValidatorQualityGateDefaults,
  buildQualityGateSchema,
};
