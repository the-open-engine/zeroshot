const assert = require('node:assert');
const path = require('node:path');

const { validateTemplates } = require('../../src/template-validation');

describe('template validation pr mode', function () {
  it('validates resolved autoPr conductor routes', async function () {
    const templatesDir = path.join(__dirname, '..', '..', 'cluster-templates');
    const report = await validateTemplates({ templatesDir, deep: false });
    const autoPrRoutes = report.results.filter((entry) =>
      entry.filePath.includes('conductor-bootstrap.json#resolved-autopr:')
    );
    const invalidRoutes = autoPrRoutes.filter((entry) => !entry.result.valid);

    assert.strictEqual(autoPrRoutes.length, 8);
    assert.strictEqual(
      invalidRoutes.length,
      0,
      invalidRoutes
        .map((entry) => `${entry.filePath}: ${entry.result.errors.join(' | ')}`)
        .join('\n')
    );
  });
});
