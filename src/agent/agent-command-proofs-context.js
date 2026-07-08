const { normalizeCommandProofs } = require('../command-proofs');

function appendProofDetails(lines, proof, index) {
  const scope = proof.scope ? `, scope: ${proof.scope}` : '';
  lines.push(`${index + 1}. id: ${proof.id}, profile: ${proof.profile}${scope}`);
  if (proof.description) {
    lines.push(`   description: ${proof.description}`);
  }
  lines.push(`   command: ${proof.command}`);
  lines.push(`   helper: zeroshot cmdproof check ${proof.id}`);
}

function buildWorkerInstructions(lines) {
  lines.push(
    '',
    'For these exact commands:',
    '- Run `zeroshot cmdproof check <id>` instead of the raw command.',
    '- Treat the helper exit code as the command exit code.',
    '- If you need to mention evidence, include the helper output and the configured command id.',
    ''
  );
}

function buildValidatorInstructions(lines) {
  lines.push(
    '',
    'For proof-backed validation:',
    '- Run `zeroshot cmdproof check <id>` before considering the raw command.',
    '- Use the helper output as quality-gate evidence.',
    '- Only run the raw command directly if the helper itself is unavailable.',
    ''
  );
}

function buildCommandProofsSection(config) {
  const proofs = normalizeCommandProofs(config.commandProofs);
  if (proofs.length === 0) {
    return '';
  }

  const lines = ['## Reusable Command Proofs', '', 'Configured proof-backed commands:'];
  proofs.forEach((proof, index) => appendProofDetails(lines, proof, index));

  if (config.role === 'validator') {
    buildValidatorInstructions(lines);
  } else {
    buildWorkerInstructions(lines);
  }

  return lines.join('\n');
}

module.exports = {
  buildCommandProofsSection,
};
