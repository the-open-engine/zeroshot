const { MAX_FRAME_BYTES } = require('./constants');

const HEADER_DELIMITER = '\r\n\r\n';

const parseContentLength = (headerText) => {
  const lines = headerText.split('\r\n');
  for (const line of lines) {
    const sepIndex = line.indexOf(':');
    if (sepIndex === -1) {
      continue;
    }
    const name = line.slice(0, sepIndex).trim().toLowerCase();
    if (name !== 'content-length') {
      continue;
    }
    const value = line.slice(sepIndex + 1).trim();
    const length = Number.parseInt(value, 10);
    if (!Number.isFinite(length) || length < 0) {
      throw new Error('Invalid Content-Length header');
    }
    return length;
  }
  throw new Error('Missing Content-Length header');
};

const createFrameParser = (options: any = {}) => {
  const maxFrameBytes =
    typeof options.maxFrameBytes === 'number' ? options.maxFrameBytes : MAX_FRAME_BYTES;
  let buffer = Buffer.alloc(0);

  const reset = () => {
    buffer = Buffer.alloc(0);
  };

  const push = (chunk) => {
    if (!chunk || chunk.length === 0) {
      return [];
    }
    buffer = Buffer.concat([buffer, chunk]);
    const frames = [];

    while (true) {
      const headerIndex = buffer.indexOf(HEADER_DELIMITER);
      if (headerIndex === -1) {
        break;
      }
      const headerText = buffer.slice(0, headerIndex).toString('utf8');
      const contentLength = parseContentLength(headerText);
      if (contentLength > maxFrameBytes) {
        throw new Error('Frame exceeds maximum size');
      }

      const totalLength = headerIndex + HEADER_DELIMITER.length + contentLength;
      if (buffer.length < totalLength) {
        break;
      }

      const payload = buffer.slice(headerIndex + HEADER_DELIMITER.length, totalLength);
      frames.push(payload.toString('utf8'));
      buffer = buffer.slice(totalLength);
    }

    return frames;
  };

  return { push, reset };
};

const encodeFrame = (payload) => {
  const payloadBuffer = Buffer.isBuffer(payload)
    ? payload
    : Buffer.from(typeof payload === 'string' ? payload : JSON.stringify(payload), 'utf8');
  if (payloadBuffer.length > MAX_FRAME_BYTES) {
    throw new Error('Frame exceeds maximum size');
  }
  const header = `Content-Length: ${payloadBuffer.length}${HEADER_DELIMITER}`;
  return Buffer.concat([Buffer.from(header, 'utf8'), payloadBuffer]);
};

module.exports = {
  createFrameParser,
  encodeFrame,
};
