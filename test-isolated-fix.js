const Orchestrator = require('./src/orchestrator');
const path = require('path');

async function test() {
  console.log('[TEST] Starting isolated mode output capture test...\n');

  const orchestrator = new Orchestrator({ quiet: false });

  const config = {
    agents: [
      {
        id: 'test-conductor',
        role: 'conductor',
        model: 'haiku',
        outputFormat: 'json',
        jsonSchema: {
          type: 'object',
          properties: {
            complexity: {
              type: 'string',
              enum: ['TRIVIAL', 'SIMPLE', 'STANDARD', 'CRITICAL'],
            },
            reasoning: { type: 'string' },
          },
          required: ['complexity', 'reasoning'],
        },
        prompt:
          'Classify this task: {{ISSUE_OPENED.content.text}}. Return JSON with complexity and reasoning.',
        triggers: [
          {
            topic: 'ISSUE_OPENED',
            action: 'execute_task',
          },
        ],
        hooks: {
          onComplete: {
            action: 'publish_message',
            config: {
              topic: 'CLASSIFICATION_DONE',
              content: {
                text: 'Classification complete',
                data: { result: '{{result}}' },
              },
            },
          },
        },
      },
    ],
  };

  try {
    console.log('[TEST] Starting cluster in isolation mode...');
    const cluster = await orchestrator.start(
      config,
      { text: 'Add a login button to the homepage' },
      { isolation: true, cwd: process.cwd() }
    );

    console.log(`[TEST] Cluster started: ${cluster.id}`);
    console.log('[TEST] Waiting for agent output...\n');

    // Wait up to 2 minutes for completion
    const timeout = 120000;
    const start = Date.now();
    let completed = false;

    while (Date.now() - start < timeout) {
      const messages = cluster.messageBus.query({
        cluster_id: cluster.id,
        topic: 'CLASSIFICATION_DONE',
      });

      if (messages.length > 0) {
        completed = true;
        console.log('[TEST] ✅ Agent completed!\n');
        break;
      }

      await new Promise((resolve) => setTimeout(resolve, 500));
    }

    if (!completed) {
      console.log('[TEST] ❌ Agent did not complete within timeout');
      await orchestrator.kill(cluster.id);
      process.exit(1);
    }

    // Check output messages
    const outputMessages = cluster.messageBus.query({
      cluster_id: cluster.id,
      topic: 'AGENT_OUTPUT',
    });

    console.log(`[TEST] Total AGENT_OUTPUT messages: ${outputMessages.length}\n`);

    // Check for help text (BAD - means we read spawn stdout instead of log file)
    const hasHelpText = outputMessages.some((m) => {
      const text = m.content?.text || m.content?.data?.line || '';
      return text.includes('zeroshot kill') || text.includes('# Stop task');
    });

    if (hasHelpText) {
      console.log('[TEST] ❌ FAILURE: Found help text in output (reading spawn stdout!)');
      await orchestrator.kill(cluster.id);
      process.exit(1);
    }

    console.log('[TEST] ✅ No help text found in output\n');

    // Check for valid JSON (GOOD - means we read the actual log file)
    let hasValidJson = false;
    let parsedOutput = null;

    for (const msg of outputMessages) {
      const text = msg.content?.text || m.content?.data?.line || '';
      if (!text.trim().startsWith('{')) continue;

      try {
        const parsed = JSON.parse(text);
        if (parsed.complexity && parsed.reasoning) {
          hasValidJson = true;
          parsedOutput = parsed;
          break;
        }
      } catch {
        // Not valid JSON, continue
      }
    }

    if (!hasValidJson) {
      console.log('[TEST] ❌ FAILURE: No valid JSON output found');
      await orchestrator.kill(cluster.id);
      process.exit(1);
    }

    console.log('[TEST] ✅ Found valid JSON output!');
    console.log('[TEST] Complexity:', parsedOutput.complexity);
    console.log('[TEST] Reasoning:', parsedOutput.reasoning.slice(0, 100) + '...\n');

    // Success!
    console.log('[TEST] ✅✅✅ ALL CHECKS PASSED ✅✅✅');
    console.log('[TEST] The fix works correctly!');

    await orchestrator.kill(cluster.id);
    process.exit(0);
  } catch (error) {
    console.error('[TEST] ❌ Error:', error.message);
    console.error(error.stack);
    process.exit(1);
  }
}

test();
