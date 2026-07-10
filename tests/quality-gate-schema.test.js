const assert = require('assert');
const Ajv = require('ajv');

const { buildQualityGateSchema } = require('../src/agent/agent-quality-gate-schema');

// Regression for #614: the quality-gate schema is injected into validator agent
// output schemas and compiled by strict-mode AJV consumers. Union
// `type: ['string','number']` arrays throw under strict mode
// ("strict mode: use allowUnionTypes ... (strictTypes)"), which crashed
// validation before the validator's output could be checked and burned
// validator retries until runs failed.
//
// The prior tests only asserted whether the schema was *injected*, never that it
// *compiles* under a strict AJV. AJV's default `new Ajv()` only LOGS strictTypes
// (it does not throw), so a naive test would pass on the buggy schema too — that
// is exactly the gap that let this ship. These tests compile with the throwing
// `{ strict: true }` config that reproduces the runtime failure, and the guard
// below proves that config actually discriminates the bug.
const STRICT = { strict: true };

describe('quality gate schema (strict AJV compatibility)', function () {
  it('guard: the strict config throws on a union `type` array (so this test can catch the regression)', function () {
    const unionSchema = { type: 'object', properties: { t: { type: ['string', 'number'] } } };
    assert.throws(() => new Ajv(STRICT).compile(unionSchema), /allowUnionTypes|strictTypes/);
  });

  it('compiles under strict AJV without strictTypes errors', function () {
    const schema = buildQualityGateSchema();
    assert.doesNotThrow(() => new Ajv(STRICT).compile(schema));
  });

  it('accepts a gate with a string completedAt/timestamp', function () {
    const validate = new Ajv(STRICT).compile(buildQualityGateSchema());
    const ok = validate([
      {
        id: 'ci',
        status: 'PASS',
        evidence: { command: 'npm test', exitCode: 0, output: 'ok' },
        completedAt: '2026-07-09T17:00:00Z',
        timestamp: '2026-07-09T17:00:00Z',
      },
    ]);
    assert.strictEqual(ok, true, JSON.stringify(validate.errors));
  });

  it('accepts a gate with a numeric completedAt/timestamp', function () {
    const validate = new Ajv(STRICT).compile(buildQualityGateSchema());
    const ok = validate([
      {
        id: 'ci',
        status: 'PASS',
        evidence: { command: 'npm test', exitCode: 0, output: 'ok' },
        completedAt: 1752080400000,
        timestamp: 1752080400000,
      },
    ]);
    assert.strictEqual(ok, true, JSON.stringify(validate.errors));
  });

  it('still rejects a wrong-typed completedAt (anyOf keeps the type constraint)', function () {
    const validate = new Ajv(STRICT).compile(buildQualityGateSchema());
    const ok = validate([
      {
        id: 'ci',
        status: 'PASS',
        evidence: { command: 'npm test', exitCode: 0, output: 'ok' },
        completedAt: true,
      },
    ]);
    assert.strictEqual(ok, false);
  });
});
