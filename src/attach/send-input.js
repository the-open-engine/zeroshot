/**
 * sendInput - write stdin data to a live attach socket
 *
 * Uses the attach protocol's STDIN message to forward input to the PTY.
 * Returns { ok, error } instead of throwing on transport failures.
 */

const net = require('net');
const fs = require('fs');
const protocol = require('./protocol');

const DEFAULT_TIMEOUT_MS = 1500;

/**
 * Send input to an attach socket via STDIN message.
 * @param {object} options
 * @param {string} options.socketPath - Unix socket path
 * @param {Buffer|string} options.data - Data to send
 * @param {number} [options.timeoutMs=1500] - Timeout in ms
 * @returns {Promise<{ok: boolean, error: string|null}>}
 */
function sendInput(options = {}) {
  const { socketPath, data, timeoutMs = DEFAULT_TIMEOUT_MS } = options;

  if (!socketPath) {
    throw new Error('sendInput: socketPath is required');
  }

  if (data === undefined || data === null) {
    throw new Error('sendInput: data is required');
  }

  if (!Number.isFinite(timeoutMs) || timeoutMs <= 0) {
    throw new Error(`sendInput: timeoutMs must be positive (got ${timeoutMs})`);
  }

  if (!fs.existsSync(socketPath)) {
    return { ok: false, error: `Socket not found: ${socketPath}` };
  }

  return new Promise((resolve) => {
    let settled = false;
    let timeout;
    const socket = net.createConnection(socketPath);

    const finish = (result) => {
      if (settled) return;
      settled = true;
      if (timeout) {
        clearTimeout(timeout);
      }
      try {
        socket.end();
        socket.destroy();
      } catch (cleanupError) {
        console.warn('[sendInput] socket cleanup failed:', cleanupError);
      }
      resolve(result);
    };

    timeout = setTimeout(() => {
      finish({ ok: false, error: 'Timeout waiting for socket connection' });
    }, timeoutMs);

    socket.on('connect', () => {
      try {
        const encoded = protocol.encode(protocol.createStdinMessage(data));
        socket.write(encoded, (err) => {
          if (err) {
            finish({ ok: false, error: err.message });
          } else {
            finish({ ok: true, error: null });
          }
        });
      } catch (err) {
        finish({ ok: false, error: err.message });
      }
    });

    socket.on('error', (err) => {
      finish({ ok: false, error: err.message });
    });
  });
}

module.exports = {
  sendInput,
};
