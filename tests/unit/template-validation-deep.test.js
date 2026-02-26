const assert = require('node:assert');
const path = require('node:path');

const { validateTemplates } = require('../../src/template-validation');

describe('Template validation (deep)', function () {
  this.timeout(10000);

  it('passes deep sim for base templates', async function () {
    const templatesDir = path.join(__dirname, '..', '..', 'cluster-templates', 'base-templates');
    const report = await validateTemplates({ templatesDir, deep: true });
    assert.strictEqual(report.valid, true);
  });

  it('validates resolved conductor topology for all classification routes', async function () {
    const templatesDir = path.join(__dirname, '..', '..', 'cluster-templates');
    const report = await validateTemplates({ templatesDir, deep: false });
    const resolvedRouteResults = report.results.filter((entry) =>
      entry.filePath.includes('conductor-bootstrap.json#resolved:')
    );

    assert.strictEqual(resolvedRouteResults.length, 12);
    const invalidRoutes = resolvedRouteResults.filter((entry) => !entry.result.valid);
    assert.strictEqual(
      invalidRoutes.length,
      0,
      invalidRoutes
        .map((entry) => `${entry.filePath}: ${entry.result.errors.join(' | ')}`)
        .join('\n')
    );
  });
});
