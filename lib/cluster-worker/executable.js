'use strict';
const { createLegacyClusterWorker } = require('./index');
const { validateCommandFrame } = require('./contracts');
const { DEFAULT_BOUNDS } = require('./profiles');
const { RELEASE_WORKER, WAIT_FOR_WORKER_CLEANUP } = require('./worker-internals');

const ERROR_CODES = Object.freeze({
  SyntaxError: 'MALFORMED_JSON',
  TypeError: 'INVALID_REQUEST',
});

function protocolError(error, fallback = 'WORKER_ERROR') {
  return Object.freeze({
    code: error?.code || ERROR_CODES[error?.name] || fallback,
    message: error instanceof Error ? error.message : String(error),
  });
}

function writeFrame(output, frame) {
  output.write(`${JSON.stringify(frame)}\n`);
}

async function pumpEvents({ id, iterator, state, output, diagnostics }) {
  try {
    for await (const event of iterator) writeFrame(output, { type: 'event', id, event });
  } catch (error) {
    diagnostics.write(`cluster-worker event stream failed: ${protocolError(error).message}\n`);
  } finally {
    state.eventPumps.delete(iterator);
  }
}

function executeCommand(frame, worker, state, streams) {
  if (frame.method === 'start') {
    if (state.startSeen) {
      const error = new Error('Worker process permits exactly one start command');
      error.code = 'DUPLICATE_START';
      throw error;
    }
    state.startSeen = true;
    state.startPromise = Promise.resolve(worker.start(frame.params.request));
    return state.startPromise;
  }
  if (frame.method === 'events') {
    const iterator = worker.events();
    state.eventPumps.add(iterator);
    pumpEvents({ id: frame.id, iterator, state, ...streams });
    return { subscribed: true };
  }
  if (state.startPromise && (frame.method === 'result' || frame.method === 'stop')) {
    const terminal = Promise.race([
      Promise.resolve().then(() => worker.result()),
      state.startPromise.then(
        () => new Promise(() => {}),
        (error) => Promise.reject(error)
      ),
    ]);
    if (frame.method === 'result') return terminal;
    return Promise.race([terminal, Promise.resolve().then(() => worker.stop())]);
  }
  return worker[frame.method]();
}

function createCommandDispatcher({ worker, output, diagnostics }) {
  const state = { eventPumps: new Set(), startSeen: false, startPromise: null };
  const streams = { output, diagnostics };

  async function dispatch(frame) {
    const id =
      frame && typeof frame === 'object' && !Array.isArray(frame) ? (frame.id ?? null) : null;
    try {
      validateCommandFrame(frame);
      const result = await executeCommand(frame, worker, state, streams);
      writeFrame(output, { type: 'response', id: frame.id, ok: true, result });
    } catch (error) {
      writeFrame(output, { type: 'response', id, ok: false, error: protocolError(error) });
    }
  }

  function closeEvents() {
    for (const iterator of [...state.eventPumps]) iterator.return();
  }

  return Object.freeze({
    dispatch,
    closeEvents,
    hasStarted: () => state.startSeen,
  });
}

function listenForFrames({ input, frameBytes, onFrame, onError, onClose }) {
  let chunks = [];
  let bytes = 0;
  let discarding = false;

  function finishLine() {
    if (discarding) {
      discarding = false;
      chunks = [];
      bytes = 0;
      return;
    }
    let line = Buffer.concat(chunks, bytes);
    if (line.at(-1) === 13) line = line.subarray(0, -1);
    chunks = [];
    bytes = 0;
    if (line.length > 0) onFrame(line.toString('utf8'));
  }

  input.on('data', (inputChunk) => {
    const chunk = Buffer.isBuffer(inputChunk) ? inputChunk : Buffer.from(inputChunk);
    let offset = 0;
    while (offset < chunk.length) {
      const newline = chunk.indexOf(10, offset);
      const end = newline === -1 ? chunk.length : newline;
      const part = chunk.subarray(offset, end);
      if (!discarding && bytes + part.length > frameBytes) {
        discarding = true;
        chunks = [];
        bytes = 0;
        onError();
      } else if (!discarding && part.length > 0) {
        chunks.push(part);
        bytes += part.length;
      }
      if (newline === -1) break;
      finishLine();
      offset = newline + 1;
    }
  });
  input.once('end', () => {
    if (bytes > 0 || discarding) finishLine();
    onClose();
  });
  input.once('error', onClose);
}

async function stopStartedWorker({ dispatcher, worker, diagnostics }) {
  if (!dispatcher.hasStarted()) return;
  await Promise.resolve(worker.stop()).catch((error) => {
    diagnostics.write(`cluster-worker stop failed: ${protocolError(error).message}\n`);
  });
  await worker[WAIT_FOR_WORKER_CLEANUP]?.();
}

function createDeadline(milliseconds) {
  let handle;
  const promise = new Promise((resolve) => {
    handle = setTimeout(resolve, milliseconds);
  });
  return { promise, cancel: () => clearTimeout(handle) };
}

function runClusterWorkerExecutable(options = {}) {
  const input = options.input || process.stdin;
  const output = options.output || process.stdout;
  const diagnostics = options.diagnostics || process.stderr;
  const worker = options.worker || createLegacyClusterWorker(options.dependencies);
  const frameBytes = options.frameBytes ?? DEFAULT_BOUNDS.frameBytes;
  const shutdownMs = options.shutdownMs ?? DEFAULT_BOUNDS.shutdownMs;
  for (const [name, value] of Object.entries({ frameBytes, shutdownMs })) {
    if (!Number.isSafeInteger(value) || value <= 0) {
      throw new TypeError(`${name} must be a positive safe integer`);
    }
  }
  const dispatcher = createCommandDispatcher({ worker, output, diagnostics });
  let closePromise = null;

  function boundedStop() {
    if (closePromise) return closePromise;
    closePromise = (async () => {
      dispatcher.closeEvents();
      const deadline = createDeadline(shutdownMs);
      await Promise.race([
        stopStartedWorker({ dispatcher, worker, diagnostics }),
        deadline.promise,
      ]);
      worker[RELEASE_WORKER]?.();
      deadline.cancel();
    })();
    return closePromise;
  }

  function rejectOversizedFrame() {
    writeFrame(output, {
      type: 'response',
      id: null,
      ok: false,
      error: { code: 'FRAME_TOO_LARGE', message: `Frame exceeds ${frameBytes} UTF-8 bytes` },
    });
  }

  function acceptLine(line) {
    if (closePromise) return;
    let frame;
    try {
      frame = JSON.parse(line);
    } catch (error) {
      writeFrame(output, {
        type: 'response',
        id: null,
        ok: false,
        error: protocolError(error),
      });
      return;
    }
    dispatcher.dispatch(frame);
  }

  listenForFrames({
    input,
    frameBytes,
    onFrame: acceptLine,
    onError: rejectOversizedFrame,
    onClose: () => boundedStop(),
  });

  return Object.freeze({
    close: boundedStop,
    dispatch: dispatcher.dispatch,
  });
}

module.exports = {
  createCommandDispatcher,
  listenForFrames,
  protocolError,
  runClusterWorkerExecutable,
};
