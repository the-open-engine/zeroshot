/**
 * AttachServer - PTY process manager with socket server for attach/detach
 *
 * Lifecycle:
 * 1. Created when task/agent spawns
 * 2. Spawns command via node-pty
 * 3. Listens on Unix socket for client connections
 * 4. Buffers output in ring buffer (for late-joining clients)
 * 5. Broadcasts output to all connected clients
 * 6. Cleans up when process exits
 *
 * Features:
 * - Multi-client support (multiple terminals can attach)
 * - Output history replay on attach
 * - Signal forwarding (SIGINT, SIGTERM)
 * - Window resize support
 * - Graceful cleanup on exit
 */

const net = require('net');
const fs = require('fs');
const path = require('path');
const EventEmitter = require('events');

const RingBuffer = require('./ring-buffer');
const protocol = require('./protocol');
const { cleanupStaleSocket } = require('./socket-discovery');

// FAIL FAST: Check for node-pty at module load time, not at spawn time
let pty;
try {
  pty = require('node-pty');
} catch (e) {
  throw new Error(
    `AttachServer: node-pty not installed. Run: npm install node-pty\n` +
      `Original error: ${e.message}`
  );
}

// Default output buffer size: 1MB
const DEFAULT_BUFFER_SIZE = 1024 * 1024;

class AttachServer extends EventEmitter {
  /**
   * @param {object} options
   * @param {string} options.id - Task or agent ID
   * @param {string} options.socketPath - Unix socket path
   * @param {string} options.command - Command to spawn
   * @param {string[]} options.args - Command arguments
   * @param {string} [options.cwd] - Working directory
   * @param {object} [options.env] - Environment variables
   * @param {number} [options.cols] - Terminal columns (default 120)
   * @param {number} [options.rows] - Terminal rows (default 30)
   * @param {number} [options.bufferSize] - Output buffer size (default 1MB)
   */
  constructor(options) {
    super();

    if (!options.id) throw new Error('AttachServer: id is required');
    if (!options.socketPath) throw new Error('AttachServer: socketPath is required');
    if (!options.command) throw new Error('AttachServer: command is required');

    this.id = options.id;
    this.socketPath = options.socketPath;
    this.command = options.command;
    this.args = options.args || [];
    this.cwd = options.cwd || process.cwd();
    this.env = options.env || process.env;
    this.cols = options.cols || 120;
    this.rows = options.rows || 30;

    this.outputBuffer = new RingBuffer(options.bufferSize || DEFAULT_BUFFER_SIZE);
    this.clients = new Map(); // clientId -> { socket, decoder }
    this.pty = null;
    this.server = null;
    this.state = 'stopped'; // stopped, starting, running, exiting, exited
    this.exitCode = null;
    this.exitSignal = null;
    this.pid = null;

    // Bind cleanup handlers
    this._onProcessExit = this._onProcessExit.bind(this);
    this._onServerError = this._onServerError.bind(this);
  }

  /**
   * Start the PTY process and socket server
   * @returns {Promise<void>}
   */
  async start() {
    if (this.state !== 'stopped') {
      throw new Error(`AttachServer: Cannot start from state '${this.state}'`);
    }

    this.state = 'starting';

    // Ensure socket directory exists
    const socketDir = path.dirname(this.socketPath);
    if (!fs.existsSync(socketDir)) {
      fs.mkdirSync(socketDir, { recursive: true });
    }

    // Clean up stale socket if exists
    await cleanupStaleSocket(this.socketPath);

    // Check if socket is still in use (another process)
    if (fs.existsSync(this.socketPath)) {
      throw new Error(`AttachServer: Socket already in use: ${this.socketPath}`);
    }

    // Start socket server FIRST (so clients can connect immediately)
    await this._startServer();

    // Spawn PTY process
    await this._spawnPty();

    this.state = 'running';
    this.emit('start', { id: this.id, pid: this.pid });
  }

  /**
   * Stop the server and kill the PTY process
   * @param {string} [signal='SIGTERM'] - Signal to send
   * @returns {Promise<void>}
   */
  async stop(signal = 'SIGTERM') {
    if (this.state !== 'running') {
      return;
    }

    this.state = 'exiting';

    // Kill PTY process
    if (this.pty) {
      try {
        this.pty.kill(signal);
      } catch {
        // Process may already be dead
      }
    }

    // Wait for process to exit (with timeout)
    await new Promise((resolve) => {
      const timeout = setTimeout(() => {
        // Force kill if still running
        if (this.pty) {
          try {
            this.pty.kill('SIGKILL');
          } catch {
            // Ignore
          }
        }
        resolve();
      }, 5000);

      const checkExit = () => {
        if (this.state === 'exited') {
          clearTimeout(timeout);
          resolve();
        } else {
          setTimeout(checkExit, 100);
        }
      };
      checkExit();
    });

    await this._cleanup();
  }

  /**
   * Send a signal to the PTY process
   * @param {string} signal - Signal name (SIGINT, SIGTERM, etc.)
   */
  sendSignal(signal) {
    if (!this.pty || this.state !== 'running') {
      return false;
    }

    try {
      this.pty.kill(signal);
      return true;
    } catch {
      return false;
    }
  }

  /**
   * Resize the PTY
   * @param {number} cols - Columns
   * @param {number} rows - Rows
   */
  resize(cols, rows) {
    if (!this.pty || this.state !== 'running') {
      return;
    }

    this.cols = cols;
    this.rows = rows;

    try {
      this.pty.resize(cols, rows);
    } catch {
      // Ignore resize errors
    }
  }

  /**
   * Write to PTY stdin (for future interactive mode)
   * @param {Buffer|string} data - Data to write
   */
  write(data) {
    if (!this.pty || this.state !== 'running') {
      return false;
    }

    try {
      this.pty.write(data);
      return true;
    } catch {
      return false;
    }
  }

  /**
   * Get current server state
   * @returns {object}
   */
  getState() {
    return {
      id: this.id,
      state: this.state,
      pid: this.pid,
      exitCode: this.exitCode,
      exitSignal: this.exitSignal,
      clientCount: this.clients.size,
      bufferSize: this.outputBuffer.getSize(),
    };
  }

  // ─────────────────────────────────────────────────────────────────
  // Private methods
  // ─────────────────────────────────────────────────────────────────

  /**
   * Start the Unix socket server
   * @private
   */
  _startServer() {
    return new Promise((resolve, reject) => {
      this.server = net.createServer((socket) => {
        this._handleClientConnection(socket);
      });

      this.server.on('error', this._onServerError);

      this.server.listen(this.socketPath, () => {
        // Set socket permissions (owner read/write only)
        try {
          fs.chmodSync(this.socketPath, 0o600);
        } catch {
          // Ignore permission errors
        }
        resolve();
      });

      this.server.on('error', (err) => {
        if (this.state === 'starting') {
          reject(err);
        }
      });
    });
  }

  /**
   * Spawn the PTY process
   * @private
   */
  _spawnPty() {
    this.pty = pty.spawn(this.command, this.args, {
      name: 'xterm-256color',
      cols: this.cols,
      rows: this.rows,
      cwd: this.cwd,
      env: this.env,
    });

    this.pid = this.pty.pid;

    // Handle PTY output
    this.pty.onData((data) => {
      this._handlePtyOutput(data);
    });

    // Handle PTY exit
    this.pty.onExit(({ exitCode, signal }) => {
      this._onProcessExit(exitCode, signal);
    });
  }

  /**
   * Handle PTY output
   * @private
   */
  _handlePtyOutput(data) {
    // Buffer output for late-joining clients
    this.outputBuffer.write(data);

    // Broadcast to all connected clients
    const message = protocol.encode(protocol.createOutputMessage(data));

    for (const [clientId, client] of this.clients) {
      try {
        client.socket.write(message);
      } catch {
        // Client disconnected, will be cleaned up
        this._removeClient(clientId);
      }
    }

    // Emit for local listeners (e.g., logging)
    this.emit('output', data);
  }

  /**
   * Handle new client connection
   * @private
   */
  _handleClientConnection(socket) {
    const decoder = new protocol.MessageDecoder();
    let clientId = null;

    socket.on('data', (data) => {
      try {
        const messages = decoder.feed(data);
        for (const msg of messages) {
          this._handleClientMessage(socket, msg, (id) => {
            clientId = id;
          });
        }
      } catch (e) {
        // Protocol error, close connection
        this._sendError(socket, `Protocol error: ${e.message}`);
        socket.end();
      }
    });

    socket.on('close', () => {
      if (clientId) {
        this._removeClient(clientId);
      }
    });

    socket.on('error', () => {
      if (clientId) {
        this._removeClient(clientId);
      }
    });
  }

  /**
   * Handle message from client
   * @private
   */
  _handleClientMessage(socket, message, setClientId) {
    switch (message.type) {
      case protocol.MessageType.ATTACH: {
        const { clientId, cols, rows } = message;
        if (!clientId) {
          this._sendError(socket, 'ATTACH requires clientId');
          return;
        }

        // Register client
        this.clients.set(clientId, {
          socket,
          decoder: new protocol.MessageDecoder(),
        });
        setClientId(clientId);

        // Send history (buffered output)
        const history = this.outputBuffer.read();
        if (history.length > 0) {
          socket.write(protocol.encode(protocol.createHistoryMessage(history)));
        }

        // Send current state
        socket.write(protocol.encode(protocol.createStateMessage(this.getState())));

        // Resize PTY to client dimensions (if provided)
        if (cols && rows) {
          this.resize(cols, rows);
        }

        this.emit('clientAttach', { clientId });
        break;
      }

      case protocol.MessageType.DETACH: {
        const { clientId } = message;
        this._removeClient(clientId);
        socket.end();
        break;
      }

      case protocol.MessageType.RESIZE: {
        const { cols, rows } = message;
        if (cols && rows) {
          this.resize(cols, rows);
        }
        break;
      }

      case protocol.MessageType.SIGNAL: {
        const { signal } = message;
        this.sendSignal(signal);
        break;
      }

      case protocol.MessageType.STDIN: {
        // Future interactive mode
        const data = protocol.decodeData(message);
        if (data) {
          this.write(data);
        }
        break;
      }

      default:
        this._sendError(socket, `Unknown message type: ${message.type}`);
    }
  }

  /**
   * Remove a client
   * @private
   */
  _removeClient(clientId) {
    const client = this.clients.get(clientId);
    if (client) {
      this.clients.delete(clientId);
      this.emit('clientDetach', { clientId });
    }
  }

  /**
   * Send error message to client
   * @private
   */
  _sendError(socket, message) {
    try {
      socket.write(protocol.encode(protocol.createErrorMessage(message)));
    } catch {
      // Client disconnected
    }
  }

  /**
   * Handle PTY process exit
   * @private
   */
  _onProcessExit(exitCode, signal) {
    this.exitCode = exitCode;
    this.exitSignal = signal;
    this.state = 'exited';

    // Notify all clients
    const exitMessage = protocol.encode(protocol.createExitMessage(exitCode, signal));

    for (const [, client] of this.clients) {
      try {
        client.socket.write(exitMessage);
        client.socket.end();
      } catch {
        // Client already disconnected
      }
    }

    this.emit('exit', { exitCode, signal });

    // Clean up after short delay (allow clients to receive exit message)
    setTimeout(() => {
      this._cleanup();
    }, 500);
  }

  /**
   * Handle server error
   * @private
   */
  _onServerError(err) {
    this.emit('error', err);
  }

  /**
   * Clean up resources
   * @private
   */
  async _cleanup() {
    // Close all client connections
    for (const [, client] of this.clients) {
      try {
        client.socket.destroy();
      } catch {
        // Ignore
      }
    }
    this.clients.clear();

    // Close server
    if (this.server) {
      await new Promise((resolve) => {
        this.server.close(() => resolve());
      });
      this.server = null;
    }

    // Remove socket file
    if (fs.existsSync(this.socketPath)) {
      try {
        fs.unlinkSync(this.socketPath);
      } catch {
        // Ignore
      }
    }

    // Clean up empty parent directory for cluster sockets
    const socketDir = path.dirname(this.socketPath);
    if (socketDir.includes('cluster-')) {
      try {
        const files = fs.readdirSync(socketDir);
        if (files.length === 0) {
          fs.rmdirSync(socketDir);
        }
      } catch {
        // Ignore
      }
    }

    this.emit('cleanup');
  }
}

module.exports = AttachServer;
