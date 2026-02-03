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
});
