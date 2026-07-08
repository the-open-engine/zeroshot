const assert = require('node:assert');
const path = require('node:path');

const { formatValidationReport } = require('../../src/template-validation/report-formatter');

describe('formatValidationReport', function () {
  const cwd = path.join(__dirname, '..', '..');
  const cleanPath = path.join(cwd, 'cluster-templates', 'base-templates', 'clean.json');
  const changedPath = path.join(cwd, 'cluster-templates', 'base-templates', 'changed.json');
  const unchangedPath = path.join(cwd, 'cluster-templates', 'base-templates', 'unchanged.json');
  const invalidPath = path.join(cwd, 'cluster-templates', 'base-templates', 'invalid.json');

  function buildReport() {
    return {
      validated: 4,
      skipped: 0,
      results: [
        { filePath: cleanPath, result: { valid: true, errors: [], warnings: [] } },
        {
          filePath: changedPath,
          result: { valid: true, errors: [], warnings: ["Agent 'a': changed warning"] },
        },
        {
          filePath: unchangedPath,
          result: {
            valid: true,
            errors: [],
            warnings: ["Agent 'b': unchanged warning 1", "Agent 'c': unchanged warning 2"],
          },
        },
        {
          filePath: invalidPath,
          result: { valid: false, errors: ['broken schema'], warnings: [] },
        },
      ],
    };
  }

  it('shows all warnings when changedFiles is null', function () {
    const { lines, hasErrors } = formatValidationReport(buildReport(), { changedFiles: null, cwd });

    const text = lines.join('\n');
    assert.ok(text.includes("WARN: Agent 'a': changed warning"));
    assert.ok(text.includes("WARN: Agent 'b': unchanged warning 1"));
    assert.ok(text.includes("WARN: Agent 'c': unchanged warning 2"));
    assert.ok(text.includes('ERROR: broken schema'));
    assert.strictEqual(hasErrors, true);
    assert.ok(text.includes('Validated: 4 templates, Skipped: 0 files'));
  });

  it('collapses warnings on unchanged templates into a summary line', function () {
    const changedRelative = path.relative(cwd, changedPath);
    const { lines, hasErrors } = formatValidationReport(buildReport(), {
      changedFiles: new Set([changedRelative]),
      cwd,
    });

    const text = lines.join('\n');
    assert.ok(text.includes("WARN: Agent 'a': changed warning"));
    assert.ok(!text.includes("WARN: Agent 'b': unchanged warning 1"));
    assert.ok(!text.includes("WARN: Agent 'c': unchanged warning 2"));
    assert.ok(text.includes('2 pre-existing warning(s) across 1 unrelated template(s)'));

    assert.ok(text.includes('ERROR: broken schema'));
    assert.strictEqual(hasErrors, true);
    assert.ok(text.includes('Validated: 4 templates, Skipped: 0 files'));
  });
});
