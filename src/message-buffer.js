/**
 * Message buffering helper
 *
 * Ensures trigger-matching messages are never dropped just because an agent/subcluster is busy.
 * Dropped workflow signals (e.g. VALIDATION_RESULT) can wedge clusters in "running" state.
 */

function bufferMessage(target, message, options = {}) {
  const maxBuffered = options.maxBuffered ?? 200;

  if (!target._bufferedMessages) {
    target._bufferedMessages = [];
  }

  if (target._bufferedMessages.length >= maxBuffered) {
    target._bufferedMessages.shift();
  }

  target._bufferedMessages.push(message);
}

function scheduleDrain(target, drainFn, options = {}) {
  if (target._bufferDrainScheduled) {
    return;
  }

  target._bufferDrainScheduled = true;

  const label = options.label || 'MessageBuffer';
  const id = target.id || 'unknown';

  const run = () => {
    target._bufferDrainScheduled = false;
    drainFn().catch((error) => {
      console.error(`\n${'='.repeat(80)}`);
      console.error(`🔴 FATAL: ${label} drain crashed (${id})`);
      console.error(`${'='.repeat(80)}`);
      console.error(`Error: ${error.message}`);
      console.error(`Stack: ${error.stack}`);
      console.error(`${'='.repeat(80)}\n`);
      setImmediate(() => {
        throw error;
      });
    });
  };

  const current = target._currentExecution;
  if (current && typeof current.finally === 'function') {
    current.finally(() => setImmediate(run));
    return;
  }

  setImmediate(run);
}

async function drainBufferedMessages(target, handleFn, options = {}) {
  if (!target.running) {
    return;
  }

  const buffer = target._bufferedMessages;
  if (!buffer || buffer.length === 0) {
    return;
  }

  if (target.state !== 'idle') {
    scheduleDrain(target, () => drainBufferedMessages(target, handleFn, options), options);
    return;
  }

  while (target.running && target.state === 'idle' && buffer.length > 0) {
    const next = buffer.shift();
    await handleFn(next);
  }
}

module.exports = {
  bufferMessage,
  scheduleDrain,
  drainBufferedMessages,
};
