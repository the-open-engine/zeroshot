const Orchestrator = require('../../src/orchestrator');
const MockTaskRunner = require('../helpers/mock-task-runner');

const storageDir = process.env.ZEROSHOT_TEST_STORAGE;
const clusterId = process.env.ZEROSHOT_TEST_CLUSTER_ID;

if (!storageDir || !clusterId) {
  console.error('Missing ZEROSHOT_TEST_STORAGE or ZEROSHOT_TEST_CLUSTER_ID');
  process.exit(1);
}

const mockRunner = new MockTaskRunner();
const orchestrator = new Orchestrator({
  quiet: true,
  storageDir,
  taskRunner: mockRunner,
});

const config = {
  agents: [
    {
      id: 'worker',
      role: 'implementation',
      timeout: 0,
      triggers: [{ topic: 'NEVER', action: 'execute_task' }],
      prompt: 'Idle agent for detached stop test',
    },
  ],
};

async function startCluster() {
  await orchestrator.start(config, { text: 'Detached stop test' }, { clusterId });
  console.log('READY');
}

async function shutdown(signal) {
  try {
    await orchestrator.stop(clusterId);
    console.log(`[DAEMON] Stopped cluster ${clusterId} from ${signal}`);
  } catch (error) {
    console.error(`[DAEMON] Failed to stop cluster ${clusterId}: ${error.message}`);
  }
  process.exit(0);
}

process.on('SIGTERM', () => shutdown('SIGTERM'));
process.on('SIGINT', () => shutdown('SIGINT'));

startCluster().catch((error) => {
  console.error(`[DAEMON] Failed to start cluster: ${error.message}`);
  console.error(error.stack);
  process.exit(1);
});

setInterval(() => {}, 1000);
