const assert = require('assert');
const fs = require('fs');
const path = require('path');

describe('CLI inspect command', function () {
  it('should register inspect command with sample-ms option', function () {
    const cliPath = path.join(__dirname, '..', '..', 'cli', 'index.js');
    const cliCode = fs.readFileSync(cliPath, 'utf8');

    assert(
      cliCode.includes(".command('inspect <id>')"),
      'inspect command should be registered in cli/index.js'
    );
    assert(
      cliCode.includes(".option('--sample-ms <ms>'"),
      'inspect command should expose --sample-ms'
    );
    assert(
      cliCode.includes('runInspectCommand'),
      'inspect command should delegate to runInspectCommand'
    );
  });
});
