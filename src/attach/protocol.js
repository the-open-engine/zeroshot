/**
 * Protocol - Message framing for attach/detach IPC
 *
 * Uses length-prefixed JSON messages over Unix sockets.
 * Format: [4-byte length (BE)] [JSON payload]
 *
 * Message Types:
 *
 * Client → Server:
 *   ATTACH    { type: 'attach', clientId, cols, rows }
 *   DETACH    { type: 'detach', clientId }
 *   RESIZE    { type: 'resize', cols, rows }
 *   SIGNAL    { type: 'signal', signal: 'SIGINT' | 'SIGTERM' }
 *   STDIN     { type: 'stdin', data: base64 }  (future interactive mode)
 *
 * Server → Client:
 *   OUTPUT    { type: 'output', data: base64, timestamp }
 *   HISTORY   { type: 'history', data: base64 }
 *   STATE     { type: 'state', status, pid, ... }
 *   EXIT      { type: 'exit', code, signal }
 *   ERROR     { type: 'error', message }
 */

const MAX_MESSAGE_SIZE = 10 * 1024 * 1024; // 10MB max message

/**
 * Encode a message for transmission
 * @param {object} message - Message object to encode
 * @returns {Buffer} - Length-prefixed encoded message
 */
function encode(message) {
  const json = JSON.stringify(message);
  const payload = Buffer.from(json, 'utf8');

  if (payload.length > MAX_MESSAGE_SIZE) {
    throw new Error(`Message too large: ${payload.length} bytes (max ${MAX_MESSAGE_SIZE})`);
  }

  const frame = Buffer.alloc(4 + payload.length);
  frame.writeUInt32BE(payload.length, 0);
  payload.copy(frame, 4);

  return frame;
}

/**
 * MessageDecoder - Streaming decoder for framed messages
 *
 * Handles partial reads and message reassembly.
 */
class MessageDecoder {
  constructor() {
    this.buffer = Buffer.alloc(0);
  }

  /**
   * Feed data into the decoder
   * @param {Buffer} data - Received data chunk
   * @returns {object[]} - Array of decoded messages (may be empty)
   */
  feed(data) {
    this.buffer = Buffer.concat([this.buffer, data]);
    const messages = [];

    while (this.buffer.length >= 4) {
      const length = this.buffer.readUInt32BE(0);

      if (length > MAX_MESSAGE_SIZE) {
        throw new Error(`Message too large: ${length} bytes (max ${MAX_MESSAGE_SIZE})`);
      }

      if (this.buffer.length < 4 + length) {
        // Incomplete message, wait for more data
        break;
      }

      const payload = this.buffer.slice(4, 4 + length);
      this.buffer = this.buffer.slice(4 + length);

      try {
        const message = JSON.parse(payload.toString('utf8'));
        messages.push(message);
      } catch (e) {
        throw new Error(`Invalid JSON in message: ${e.message}`);
      }
    }

    return messages;
  }

  /**
   * Reset decoder state
   */
  reset() {
    this.buffer = Buffer.alloc(0);
  }
}

// Message type constants
const MessageType = {
  // Client → Server
  ATTACH: 'attach',
  DETACH: 'detach',
  RESIZE: 'resize',
  SIGNAL: 'signal',
  STDIN: 'stdin',

  // Server → Client
  OUTPUT: 'output',
  HISTORY: 'history',
  STATE: 'state',
  EXIT: 'exit',
  ERROR: 'error',
};

// Helper functions to create messages

/**
 * Create an ATTACH message
 */
function createAttachMessage(clientId, cols, rows) {
  return { type: MessageType.ATTACH, clientId, cols, rows };
}

/**
 * Create a DETACH message
 */
function createDetachMessage(clientId) {
  return { type: MessageType.DETACH, clientId };
}

/**
 * Create a RESIZE message
 */
function createResizeMessage(cols, rows) {
  return { type: MessageType.RESIZE, cols, rows };
}

/**
 * Create a SIGNAL message
 */
function createSignalMessage(signal) {
  if (!['SIGINT', 'SIGTERM', 'SIGKILL', 'SIGTSTP'].includes(signal)) {
    throw new Error(`Invalid signal: ${signal}`);
  }
  return { type: MessageType.SIGNAL, signal };
}

/**
 * Create a STDIN message (for future interactive mode)
 */
function createStdinMessage(data) {
  const buf = Buffer.isBuffer(data) ? data : Buffer.from(data);
  return { type: MessageType.STDIN, data: buf.toString('base64') };
}

/**
 * Create an OUTPUT message
 */
function createOutputMessage(data, timestamp = Date.now()) {
  const buf = Buffer.isBuffer(data) ? data : Buffer.from(data);
  return { type: MessageType.OUTPUT, data: buf.toString('base64'), timestamp };
}

/**
 * Create a HISTORY message
 */
function createHistoryMessage(data) {
  const buf = Buffer.isBuffer(data) ? data : Buffer.from(data);
  return { type: MessageType.HISTORY, data: buf.toString('base64') };
}

/**
 * Create a STATE message
 */
function createStateMessage(state) {
  return { type: MessageType.STATE, ...state };
}

/**
 * Create an EXIT message
 */
function createExitMessage(code, signal) {
  return { type: MessageType.EXIT, code, signal };
}

/**
 * Create an ERROR message
 */
function createErrorMessage(message) {
  return { type: MessageType.ERROR, message };
}

/**
 * Decode base64 data field from OUTPUT/HISTORY/STDIN messages
 */
function decodeData(message) {
  if (message.data) {
    return Buffer.from(message.data, 'base64');
  }
  return null;
}

module.exports = {
  encode,
  MessageDecoder,
  MessageType,
  createAttachMessage,
  createDetachMessage,
  createResizeMessage,
  createSignalMessage,
  createStdinMessage,
  createOutputMessage,
  createHistoryMessage,
  createStateMessage,
  createExitMessage,
  createErrorMessage,
  decodeData,
  MAX_MESSAGE_SIZE,
};
