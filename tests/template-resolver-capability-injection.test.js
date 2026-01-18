const { describe, it, beforeEach, afterEach } = require('node:test');
const assert = require('node:assert');
const fs = require('fs');
const path = require('path');
const os = require('os');
const TemplateResolver = require('../src/template-resolver');

describe('TemplateResolver - Capability Injection', () => {
  let tempDir;
  let templatesDir;
  let baseTemplatesDir;
  let extensionsDir;
  let resolver;

  beforeEach(() => {
    // Create temp directory structure
    tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'template-resolver-test-'));
    templatesDir = tempDir;
    baseTemplatesDir = path.join(tempDir, 'base-templates');
    extensionsDir = path.join(tempDir, 'capability-extensions');

    fs.mkdirSync(baseTemplatesDir);
    fs.mkdirSync(extensionsDir);

    // Create test capability extensions
    fs.writeFileSync(
      path.join(extensionsDir, 'sub-agents.md'),
      '## SUB-AGENT DELEGATION\nUse Task tool for parallel execution.'
    );
    fs.writeFileSync(
      path.join(extensionsDir, 'parallel-execution.md'),
      '## PARALLEL TOOLS\nExecute multiple tools simultaneously.'
    );

    // Create test template with capability marker
    const testTemplate = {
      name: 'test-workflow',
      description: 'Test template with capability injection',
      params: {
        complexity: { type: 'string', default: 'STANDARD' },
      },
      agents: [
        {
          id: 'worker',
          role: 'implementation',
          modelLevel: 'level2',
          prompt: {
            initial: 'You are a worker.\n\n{{CAPABILITY_EXTENSIONS}}\n\nComplete the task.',
            subsequent: 'Continue working.\n\n{{CAPABILITY_EXTENSIONS}}\n\nFinish up.',
          },
        },
        {
          id: 'validator',
          role: 'validator',
          modelLevel: 'level1',
          prompt: 'Validate the work. No capabilities needed here.',
        },
      ],
    };

    fs.writeFileSync(
      path.join(baseTemplatesDir, 'test-workflow.json'),
      JSON.stringify(testTemplate, null, 2)
    );

    resolver = new TemplateResolver(templatesDir);
  });

  afterEach(() => {
    // Cleanup temp directory
    if (tempDir && fs.existsSync(tempDir)) {
      fs.rmSync(tempDir, { recursive: true, force: true });
    }
  });

  it('should inject capabilities when provider specified', () => {
    const resolved = resolver.resolve(
      'test-workflow',
      { complexity: 'STANDARD' },
      { provider: 'claude' }
    );

    const workerAgent = resolved.agents.find((a) => a.id === 'worker');
    assert.ok(workerAgent, 'Should have worker agent');
    assert.ok(
      workerAgent.prompt.initial.includes('SUB-AGENT DELEGATION'),
      'Should inject sub-agent extension in initial prompt'
    );
    assert.ok(
      workerAgent.prompt.subsequent.includes('SUB-AGENT DELEGATION'),
      'Should inject sub-agent extension in subsequent prompt'
    );
    assert.ok(
      !workerAgent.prompt.initial.includes('{{CAPABILITY_EXTENSIONS}}'),
      'Should remove marker'
    );
  });

  it('should NOT inject sub-agents for Gemini', () => {
    const resolved = resolver.resolve(
      'test-workflow',
      { complexity: 'STANDARD' },
      { provider: 'gemini' }
    );

    const workerAgent = resolved.agents.find((a) => a.id === 'worker');
    assert.ok(workerAgent, 'Should have worker agent');
    assert.ok(
      !workerAgent.prompt.initial.includes('SUB-AGENT DELEGATION'),
      'Should NOT inject sub-agent extension'
    );
    assert.ok(
      workerAgent.prompt.initial.includes('PARALLEL TOOLS'),
      'Should inject parallel execution extension'
    );
    assert.ok(
      !workerAgent.prompt.initial.includes('{{CAPABILITY_EXTENSIONS}}'),
      'Should remove marker'
    );
  });

  it('should work without provider option (backward compatibility)', () => {
    const resolved = resolver.resolve('test-workflow', { complexity: 'STANDARD' });

    const workerAgent = resolved.agents.find((a) => a.id === 'worker');
    assert.ok(workerAgent, 'Should have worker agent');
    // Marker should remain since no injection happened
    assert.ok(
      workerAgent.prompt.initial.includes('{{CAPABILITY_EXTENSIONS}}'),
      'Should keep marker when no provider specified'
    );
  });

  it('should inject into string prompts', () => {
    // Create template with string prompt
    const stringPromptTemplate = {
      name: 'string-prompt-test',
      agents: [
        {
          id: 'agent1',
          role: 'worker',
          modelLevel: 'level2',
          prompt: 'Simple string prompt.\n\n{{CAPABILITY_EXTENSIONS}}\n\nEnd.',
        },
      ],
    };

    fs.writeFileSync(
      path.join(baseTemplatesDir, 'string-prompt-test.json'),
      JSON.stringify(stringPromptTemplate, null, 2)
    );

    const resolved = resolver.resolve('string-prompt-test', {}, { provider: 'claude' });

    const agent = resolved.agents[0];
    assert.ok(typeof agent.prompt === 'string', 'Should preserve string prompt type');
    assert.ok(agent.prompt.includes('SUB-AGENT DELEGATION'), 'Should inject capabilities');
    assert.ok(!agent.prompt.includes('{{CAPABILITY_EXTENSIONS}}'), 'Should remove marker');
  });

  it('should inject into all prompt fields', () => {
    // Create template with multiple prompt fields
    const multiFieldTemplate = {
      name: 'multi-field-test',
      agents: [
        {
          id: 'agent1',
          role: 'worker',
          modelLevel: 'level2',
          prompt: {
            system: 'System prompt.\n\n{{CAPABILITY_EXTENSIONS}}',
            initial: 'Initial prompt.\n\n{{CAPABILITY_EXTENSIONS}}',
            subsequent: 'Subsequent prompt.\n\n{{CAPABILITY_EXTENSIONS}}',
          },
        },
      ],
    };

    fs.writeFileSync(
      path.join(baseTemplatesDir, 'multi-field-test.json'),
      JSON.stringify(multiFieldTemplate, null, 2)
    );

    const resolved = resolver.resolve('multi-field-test', {}, { provider: 'claude' });

    const agent = resolved.agents[0];
    assert.ok(agent.prompt.system.includes('SUB-AGENT DELEGATION'), 'Should inject in system');
    assert.ok(agent.prompt.initial.includes('SUB-AGENT DELEGATION'), 'Should inject in initial');
    assert.ok(
      agent.prompt.subsequent.includes('SUB-AGENT DELEGATION'),
      'Should inject in subsequent'
    );
    assert.ok(
      !agent.prompt.system.includes('{{CAPABILITY_EXTENSIONS}}'),
      'Should remove all markers'
    );
  });

  it('should handle agents without prompts', () => {
    const noPromptTemplate = {
      name: 'no-prompt-test',
      agents: [
        {
          id: 'agent1',
          role: 'worker',
          modelLevel: 'level2',
          // No prompt field
        },
      ],
    };

    fs.writeFileSync(
      path.join(baseTemplatesDir, 'no-prompt-test.json'),
      JSON.stringify(noPromptTemplate, null, 2)
    );

    // Should not throw
    const resolved = resolver.resolve('no-prompt-test', {}, { provider: 'claude' });
    assert.ok(resolved.agents, 'Should resolve successfully');
    assert.strictEqual(resolved.agents[0].prompt, undefined, 'Should preserve missing prompt');
  });

  it('should not break verification of other placeholders', () => {
    const unresolvedTemplate = {
      name: 'unresolved-test',
      params: {
        required_param: { type: 'string' },
      },
      agents: [
        {
          id: 'agent1',
          role: 'worker',
          modelLevel: 'level2',
          prompt: 'Test {{required_param}} and {{CAPABILITY_EXTENSIONS}}',
        },
      ],
    };

    fs.writeFileSync(
      path.join(baseTemplatesDir, 'unresolved-test.json'),
      JSON.stringify(unresolvedTemplate, null, 2)
    );

    // Should throw for missing required_param
    assert.throws(
      () => resolver.resolve('unresolved-test', {}, { provider: 'claude' }),
      /Missing required params/,
      'Should still validate required params'
    );

    // Should succeed with param provided
    const resolved = resolver.resolve(
      'unresolved-test',
      { required_param: 'value' },
      { provider: 'claude' }
    );
    assert.ok(resolved.agents[0].prompt.includes('value'), 'Should resolve regular params');
    assert.ok(resolved.agents[0].prompt.includes('SUB-AGENT'), 'Should inject capabilities');
  });
});
