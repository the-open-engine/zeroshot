const assert = require('assert');
const fs = require('fs');
const path = require('path');

describe('CLI resume command', function () {
  it('should load clusters before checking for a task fallback', function () {
    const cliPath = path.join(__dirname, '..', '..', 'cli', 'index.js');
    const cliCode = fs.readFileSync(cliPath, 'utf8');

    const resumeStart = cliCode.indexOf(".command('resume");
    assert(resumeStart !== -1, 'resume command not found in cli/index.js');

    const resumeEnd = cliCode.indexOf(".command('finish", resumeStart);
    const resumeBlock = cliCode.slice(resumeStart, resumeEnd === -1 ? cliCode.length : resumeEnd);

    const usesCreate =
      resumeBlock.includes('OrchestratorModule.create') || resumeBlock.includes('getOrchestrator(');

    assert(
      usesCreate,
      'resume command should load clusters via Orchestrator.create or getOrchestrator'
    );
  });
});
