// Jest unit tests run in CommonJS; use global require for CJS modules.
// eslint-disable-next-line @typescript-eslint/no-var-requires
const AgentWrapper = require('../../../zeroshot/cluster/src/agent-wrapper.js');
// eslint-disable-next-line @typescript-eslint/no-var-requires
const ClaudeTaskRunner = require('../../../zeroshot/cluster/src/claude-task-runner.js');

describe('Vibe live logs with jsonSchema', () => {
  test('AgentWrapper runs stream-json when jsonSchema + outputFormat=json', async () => {
    const captured: string[][] = [];

    const config = {
      id: 'planner',
      role: 'planning',
      modelLevel: 'level2',
      outputFormat: 'json',
      jsonSchema: {
        type: 'object',
        properties: { plan: { type: 'string' } },
        required: ['plan'],
      },
      triggers: [{ topic: 'ISSUE_OPENED', action: 'execute_task' }],
    };

    const messageBusStub = {
      publish: jest.fn(),
      query: jest.fn(() => []),
      findLast: jest.fn(() => null),
      count: jest.fn(() => 0),
      since: jest.fn(() => []),
      subscribe: jest.fn(),
    };

    const clusterStub = { id: 'cluster-1', createdAt: Date.now() };

    const agent = new AgentWrapper(config, messageBusStub, clusterStub, {
      testMode: true,
      quiet: true,
      mockSpawnFn: async (args: string[]) => {
        captured.push(args);
        return { success: true, output: '', error: null };
      },
    });

    await agent._spawnClaudeTask('hello');

    const args = captured[0];
    const idx = args.indexOf('--output-format');
    expect(idx).toBeGreaterThan(-1);
    expect(args[idx + 1]).toBe('stream-json');
    expect(args).not.toContain('--json-schema');
  });

  test('ClaudeTaskRunner runs stream-json when jsonSchema + outputFormat=json', async () => {
    const runner = new ClaudeTaskRunner({ quiet: true });
    let capturedArgs: string[] | null = null;

    // Override internal methods to avoid spawning real tasks
    (runner as any)._spawnAndGetTaskId = jest.fn(async (_ctPath: string, args: string[]) => {
      capturedArgs = args;
      return 'task-test-1';
    });
    (runner as any)._waitForTaskReady = jest.fn(async () => {});
    (runner as any)._followLogs = jest.fn(async () => ({
      success: true,
      output: '',
      error: null,
      taskId: 'task-test-1',
    }));

    await runner.run('ctx', {
      agentId: 'planner',
      modelLevel: 'level2',
      outputFormat: 'json',
      jsonSchema: {
        type: 'object',
        properties: { plan: { type: 'string' } },
        required: ['plan'],
      },
    });

    expect(capturedArgs).not.toBeNull();
    const args = capturedArgs as string[];
    const idx = args.indexOf('--output-format');
    expect(idx).toBeGreaterThan(-1);
    expect(args[idx + 1]).toBe('stream-json');
    expect(args).not.toContain('--json-schema');
  });

  test('AgentWrapper uses json output when strictSchema=true', async () => {
    const captured: string[][] = [];

    const config = {
      id: 'planner',
      role: 'planning',
      modelLevel: 'level2',
      outputFormat: 'json',
      strictSchema: true, // <-- This should force json output
      jsonSchema: {
        type: 'object',
        properties: { plan: { type: 'string' } },
        required: ['plan'],
      },
      triggers: [{ topic: 'ISSUE_OPENED', action: 'execute_task' }],
    };

    const messageBusStub = {
      publish: jest.fn(),
      query: jest.fn(() => []),
      findLast: jest.fn(() => null),
      count: jest.fn(() => 0),
      since: jest.fn(() => []),
      subscribe: jest.fn(),
    };

    const clusterStub = { id: 'cluster-1', createdAt: Date.now() };

    const agent = new AgentWrapper(config, messageBusStub, clusterStub, {
      testMode: true,
      quiet: true,
      mockSpawnFn: async (args: string[]) => {
        captured.push(args);
        return { success: true, output: '', error: null };
      },
    });

    await agent._spawnClaudeTask('hello');

    const args = captured[0];
    const idx = args.indexOf('--output-format');
    expect(idx).toBeGreaterThan(-1);
    expect(args[idx + 1]).toBe('json'); // NOT stream-json
    expect(args).toContain('--json-schema'); // Schema passed to CLI
  });

  test('ClaudeTaskRunner uses json output when strictSchema=true', async () => {
    const runner = new ClaudeTaskRunner({ quiet: true });
    let capturedArgs: string[] | null = null;

    (runner as any)._spawnAndGetTaskId = jest.fn(async (_ctPath: string, args: string[]) => {
      capturedArgs = args;
      return 'task-test-1';
    });
    (runner as any)._waitForTaskReady = jest.fn(async () => {});
    (runner as any)._followLogs = jest.fn(async () => ({
      success: true,
      output: '',
      error: null,
      taskId: 'task-test-1',
    }));

    await runner.run('ctx', {
      agentId: 'planner',
      modelLevel: 'level2',
      outputFormat: 'json',
      strictSchema: true, // <-- This should force json output
      jsonSchema: {
        type: 'object',
        properties: { plan: { type: 'string' } },
        required: ['plan'],
      },
    });

    expect(capturedArgs).not.toBeNull();
    const args = capturedArgs as string[];
    const idx = args.indexOf('--output-format');
    expect(idx).toBeGreaterThan(-1);
    expect(args[idx + 1]).toBe('json'); // NOT stream-json
    expect(args).toContain('--json-schema'); // Schema passed to CLI
  });

  test('Non-validator schema mismatch warns but does not throw', async () => {
    const config = {
      id: 'planner',
      role: 'planning',
      modelLevel: 'level2',
      outputFormat: 'json',
      jsonSchema: {
        type: 'object',
        properties: { plan: { type: 'string' } },
        required: ['plan'],
      },
      triggers: [],
    };

    const messageBusStub = {
      publish: jest.fn(),
      query: jest.fn(() => []),
      findLast: jest.fn(() => null),
      count: jest.fn(() => 0),
      since: jest.fn(() => []),
      subscribe: jest.fn(),
    };

    const clusterStub = { id: 'cluster-1', createdAt: Date.now() };
    const agent = new AgentWrapper(config, messageBusStub, clusterStub, {
      testMode: true,
      quiet: true,
      mockSpawnFn: async () => ({ success: true, output: '', error: null }),
    });

    const badOutput = `{\"type\":\"result\",\"structured_output\":{\"wrong\":1}}`;
    // Should resolve without throwing (async function)
    await expect(agent._parseResultOutput(badOutput)).resolves.toBeDefined();
    expect(messageBusStub.publish).toHaveBeenCalledWith(
      expect.objectContaining({ topic: 'AGENT_SCHEMA_WARNING' })
    );
  });
});
