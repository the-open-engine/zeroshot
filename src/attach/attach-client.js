/**
 * AttachClient - Terminal client for attaching to running tasks/agents
 *
 * Key sequences:
 * - Ctrl+C: Clean detach (return to shell, task continues running)
 * - Ctrl+B d: Also detach (for tmux muscle memory)
 * - Ctrl+B ?: Show help
 * - Ctrl+B c: Send SIGINT to process (interrupt agent - USE WITH CAUTION)
 * - Ctrl+Z: Forward SIGTSTP to process
 *
 * Features:
 * - Raw terminal mode (passes through all input)
 * - Output history replay on attach
 * - Terminal resize forwarding
 * - Graceful cleanup on exit
 */

const net = require('net');
const EventEmitter = require('events');
const crypto = require('crypto');

const protocol = require('./protocol');

// Key codes
const CTRL_B = '\x02';
const CTRL_C = '\x03';
const CTRL_Z = '\x1a';

// Detach timeout (ms) - how long to wait for second key after Ctrl+B
const DETACH_TIMEOUT = 500;

class AttachClient extends EventEmitter {
  /**
   * @param {object} options
   * @param {string} options.socketPath - Unix socket path to connect to
   * @param {object} [options.stdin] - Input stream (default process.stdin)
   * @param {object} [options.stdout] - Output stream (default process.stdout)
   */
  constructor(options) {
    super();

    if (!options.socketPath) {
      throw new Error('AttachClient: socketPath is required');
    }

    this.socketPath = options.socketPath;
    this.stdin = options.stdin || process.stdin;
    this.stdout = options.stdout || process.stdout;

    this.clientId = crypto.randomUUID();
    this.socket = null;
    this.decoder = new protocol.MessageDecoder();
    this.connected = false;
    this.wasRawMode = null;

    // Ctrl+B sequence detection
    this.ctrlBPressed = false;
    this.ctrlBTimeout = null;

    // Bind handlers
    this._onSocketData = this._onSocketData.bind(this);
    this._onSocketClose = this._onSocketClose.bind(this);
    this._onSocketError = this._onSocketError.bind(this);
    this._onStdinData = this._onStdinData.bind(this);
    this._onResize = this._onResize.bind(this);
  }

  /**
   * Connect to the attach server
   * @returns {Promise<void>}
   */
  connect() {
    if (this.connected) {
      throw new Error('AttachClient: Already connected');
    }

    return new Promise((resolve, reject) => {
      this.socket = net.createConnection(this.socketPath);

      this.socket.on('connect', () => {
        this.connected = true;

        // Send attach message with terminal dimensions
        const cols = this.stdout.columns || 80;
        const rows = this.stdout.rows || 24;

        this.socket.write(protocol.encode(protocol.createAttachMessage(this.clientId, cols, rows)));

        // Set up terminal
        this._setupTerminal();

        // Set up socket handlers
        this.socket.on('data', this._onSocketData);
        this.socket.on('close', this._onSocketClose);
        this.socket.on('error', this._onSocketError);

        resolve();
      });

      this.socket.on('error', (err) => {
        if (!this.connected) {
          reject(err);
        }
      });

      // Connection timeout
      const timeout = setTimeout(() => {
        if (!this.connected) {
          this.socket.destroy();
          reject(new Error('Connection timeout'));
        }
      }, 5000);

      this.socket.on('connect', () => clearTimeout(timeout));
    });
  }

  /**
   * Disconnect from the server
   */
  disconnect() {
    if (!this.connected) {
      return;
    }

    // Send detach message
    try {
      this.socket.write(protocol.encode(protocol.createDetachMessage(this.clientId)));
    } catch {
      // Ignore
    }

    this._cleanup();
    this.emit('detach');
  }

  /**
   * Send a signal to the remote process
   * @param {string} signal - Signal name
   */
  sendSignal(signal) {
    if (!this.connected) {
      return;
    }

    try {
      this.socket.write(protocol.encode(protocol.createSignalMessage(signal)));
    } catch {
      // Ignore
    }
  }

  // ─────────────────────────────────────────────────────────────────
  // Private methods
  // ─────────────────────────────────────────────────────────────────

  /**
   * Set up terminal for raw mode
   * @private
   */
  _setupTerminal() {
    // Enable raw mode if stdin is a TTY
    if (this.stdin.isTTY && this.stdin.setRawMode) {
      this.wasRawMode = this.stdin.isRaw;
      this.stdin.setRawMode(true);
    }

    // Resume stdin (may be paused)
    this.stdin.resume();

    // Listen for input
    this.stdin.on('data', this._onStdinData);

    // Listen for resize events
    if (this.stdout.isTTY) {
      this.stdout.on('resize', this._onResize);
    }

    // Handle process signals for cleanup
    process.on('SIGINT', () => {
      // Clean detach on Ctrl+C - task continues running
      this.disconnect();
    });

    process.on('SIGTERM', () => {
      this._cleanup();
      process.exit(0);
    });
  }

  /**
   * Restore terminal state
   * @private
   */
  _restoreTerminal() {
    // Restore raw mode
    if (this.stdin.isTTY && this.stdin.setRawMode && this.wasRawMode !== null) {
      this.stdin.setRawMode(this.wasRawMode);
    }

    // Remove listeners
    this.stdin.removeListener('data', this._onStdinData);
    if (this.stdout.isTTY) {
      this.stdout.removeListener('resize', this._onResize);
    }

    // Pause stdin
    this.stdin.pause();
  }

  /**
   * Handle data from socket
   * @private
   */
  _onSocketData(data) {
    try {
      const messages = this.decoder.feed(data);
      for (const msg of messages) {
        this._handleMessage(msg);
      }
    } catch (e) {
      this.emit('error', new Error(`Protocol error: ${e.message}`));
      this._cleanup();
    }
  }

  /**
   * Handle message from server
   * @private
   */
  _handleMessage(message) {
    switch (message.type) {
      case protocol.MessageType.OUTPUT: {
        const data = protocol.decodeData(message);
        if (data) {
          this.stdout.write(data);
        }
        break;
      }

      case protocol.MessageType.HISTORY: {
        const data = protocol.decodeData(message);
        if (data) {
          this.stdout.write(data);
        }
        break;
      }

      case protocol.MessageType.STATE: {
        this.emit('state', message);
        break;
      }

      case protocol.MessageType.EXIT: {
        const { code, signal } = message;
        this.emit('exit', { code, signal });
        this._cleanup();
        break;
      }

      case protocol.MessageType.ERROR: {
        this.emit('error', new Error(message.message));
        break;
      }
    }
  }

  /**
   * Handle stdin data
   * @private
   */
  _onStdinData(data) {
    const str = data.toString();

    // Handle Ctrl+B sequence
    if (this.ctrlBPressed) {
      this.ctrlBPressed = false;
      if (this.ctrlBTimeout) {
        clearTimeout(this.ctrlBTimeout);
        this.ctrlBTimeout = null;
      }

      // Check for command keys
      if (str === 'd' || str === 'D') {
        // Detach
        this.disconnect();
        return;
      }

      if (str === 'c' || str === 'C') {
        // Send SIGINT to process (interrupt agent - USE WITH CAUTION)
        this.stdout.write('\r\n⚠️  Sending SIGINT to agent (interrupting task)...\r\n');
        this.sendSignal('SIGINT');
        return;
      }

      if (str === '?') {
        // Show help
        this._showHelp();
        return;
      }

      // Not a recognized command, forward Ctrl+B + this key
      this._forwardInput(Buffer.from([0x02]));
      this._forwardInput(data);
      return;
    }

    // Check for Ctrl+B
    if (str === CTRL_B) {
      this.ctrlBPressed = true;

      // Set timeout to forward if no follow-up key
      this.ctrlBTimeout = setTimeout(() => {
        if (this.ctrlBPressed) {
          this.ctrlBPressed = false;
          this._forwardInput(data);
        }
      }, DETACH_TIMEOUT);
      return;
    }

    // Check for Ctrl+C - clean detach (task continues running)
    if (str === CTRL_C) {
      this.disconnect();
      return;
    }

    // Check for Ctrl+Z
    if (str === CTRL_Z) {
      this.sendSignal('SIGTSTP');
      return;
    }

    // Forward other input (future interactive mode)
    this._forwardInput(data);
  }

  /**
   * Forward input to remote process
   * @private
   */
  _forwardInput(data) {
    if (!this.connected) {
      return;
    }

    try {
      this.socket.write(protocol.encode(protocol.createStdinMessage(data)));
    } catch {
      // Ignore
    }
  }

  /**
   * Handle terminal resize
   * @private
   */
  _onResize() {
    if (!this.connected) {
      return;
    }

    const cols = this.stdout.columns;
    const rows = this.stdout.rows;

    try {
      this.socket.write(protocol.encode(protocol.createResizeMessage(cols, rows)));
    } catch {
      // Ignore
    }
  }

  /**
   * Handle socket close
   * @private
   */
  _onSocketClose() {
    if (this.connected) {
      this.emit('close');
      this._cleanup();
    }
  }

  /**
   * Handle socket error
   * @private
   */
  _onSocketError(err) {
    this.emit('error', err);
    this._cleanup();
  }

  /**
   * Show help message
   * @private
   */
  _showHelp() {
    const help = `
\r\n╭──────────────────────────────────────────────────────────╮
\r\n│              Vibe Attach - Key Bindings                  │
\r\n├──────────────────────────────────────────────────────────┤
\r\n│  Ctrl+C      Detach (task continues running)             │
\r\n│  Ctrl+B d    Also detach (for tmux muscle memory)        │
\r\n│  Ctrl+B ?    Show this help                              │
\r\n│  Ctrl+B c    ⚠️  Interrupt agent (sends SIGINT)           │
\r\n│  Ctrl+Z      Suspend process (sends SIGTSTP)             │
\r\n╰──────────────────────────────────────────────────────────╯
\r\n`;
    this.stdout.write(help);
  }

  /**
   * Clean up resources
   * @private
   */
  _cleanup() {
    this.connected = false;

    // Clear Ctrl+B timeout
    if (this.ctrlBTimeout) {
      clearTimeout(this.ctrlBTimeout);
      this.ctrlBTimeout = null;
    }

    // Restore terminal
    this._restoreTerminal();

    // Close socket
    if (this.socket) {
      this.socket.removeAllListeners();
      this.socket.destroy();
      this.socket = null;
    }
  }
}

module.exports = AttachClient;
