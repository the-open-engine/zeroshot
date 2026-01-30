const fs = require('fs');
const path = require('path');
const { expect } = require('chai');

require.extensions['.ts'] = require.extensions['.js'];

const {
  createValidator,
  RPC_ERROR_CODES,
} = require('../../../src/tui-backend/protocol/validator.ts');

const fixturesDir = path.join(__dirname, '..', '..', 'fixtures', 'tui-v2', 'protocol');

const readFixture = (file) => {
  const raw = fs.readFileSync(path.join(fixturesDir, file), 'utf8');
  return JSON.parse(raw);
};

const listFixtures = () =>
  fs
    .readdirSync(fixturesDir)
    .filter((file) => file.endsWith('.json'))
    .sort();

const extractMethod = (file) => {
  const parts = file.split('.');
  if (parts.length < 3) {
    return null;
  }
  return parts[1];
};

describe('tui-v2 protocol validation', () => {
  const validator = createValidator();

  it('accepts all valid request fixtures', () => {
    for (const file of listFixtures()) {
      if (!file.startsWith('request.')) continue;
      const method = extractMethod(file);
      const payload = readFixture(file);
      const result = validator.validateRequest(payload);
      expect(result.ok, `${file} failed: ${JSON.stringify(result.error)}`).to.equal(true);
      expect(payload.method).to.equal(method);
    }
  });

  it('accepts all valid response fixtures', () => {
    for (const file of listFixtures()) {
      if (!file.startsWith('response.')) continue;
      const method = extractMethod(file);
      const payload = readFixture(file);
      const result = validator.validateResponse(payload, method);
      expect(result.ok, `${file} failed: ${JSON.stringify(result.error)}`).to.equal(true);
    }
  });

  it('accepts all valid notification fixtures', () => {
    for (const file of listFixtures()) {
      if (!file.startsWith('notification.')) continue;
      const method = extractMethod(file);
      const payload = readFixture(file);
      const result = validator.validateNotification(payload);
      expect(result.ok, `${file} failed: ${JSON.stringify(result.error)}`).to.equal(true);
      expect(payload.method).to.equal(method);
    }
  });

  it('rejects invalid fixtures with structured RPC errors', () => {
    for (const file of listFixtures()) {
      if (!file.startsWith('invalid.')) continue;
      const payload = readFixture(file);
      const result = validator.validateRequest(payload);
      expect(result.ok, `${file} unexpectedly ok`).to.equal(false);
      expect(result.error).to.be.an('object');

      if (file.startsWith('invalid.request.')) {
        expect(result.error.code).to.equal(RPC_ERROR_CODES.INVALID_REQUEST);
      } else if (file.startsWith('invalid.params.')) {
        expect(result.error.code).to.equal(RPC_ERROR_CODES.INVALID_PARAMS);
      }
    }
  });
});
