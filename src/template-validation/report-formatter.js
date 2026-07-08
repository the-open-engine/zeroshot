const path = require('node:path');

function formatEntry({ relativePath, result, changedFiles, lines, suppressed }) {
  if (!result.valid) {
    lines.push(`\n❌ ${relativePath}`);
    for (const error of result.errors) {
      lines.push(`   ERROR: ${error}`);
    }
    return true;
  }

  if (result.warnings.length > 0) {
    if (changedFiles === null || changedFiles.has(relativePath)) {
      lines.push(`\n⚠️  ${relativePath}`);
      for (const warning of result.warnings) {
        lines.push(`   WARN: ${warning}`);
      }
    } else {
      suppressed.count += result.warnings.length;
      suppressed.fileCount += 1;
    }
    return false;
  }

  lines.push(`✓ ${relativePath}`);
  return false;
}

function formatValidationReport(report, { changedFiles = null, cwd = process.cwd() } = {}) {
  const lines = [];
  const suppressed = { count: 0, fileCount: 0 };
  let hasErrors = false;

  for (const { filePath, result } of report.results) {
    const relativePath = path.relative(cwd, filePath);
    if (formatEntry({ relativePath, result, changedFiles, lines, suppressed })) {
      hasErrors = true;
    }
  }

  if (suppressed.count > 0) {
    lines.push(
      `\n${suppressed.count} pre-existing warning(s) across ${suppressed.fileCount} unrelated template(s) — run \`npm run validate:templates\` for detail`
    );
  }

  lines.push(`\n${'='.repeat(60)}`);
  lines.push(`Validated: ${report.validated} templates, Skipped: ${report.skipped} files`);

  return { lines, hasErrors };
}

module.exports = {
  formatValidationReport,
};
