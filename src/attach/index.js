/**
 * Attach/Detach System - tmux-style session management for zeroshot tasks and clusters
 *
 * Usage:
 *   const { AttachServer, AttachClient, socketDiscovery } = require('./attach');
 *
 *   // Server side (in watcher.js or agent-wrapper.js)
 *   const server = new AttachServer({
 *     id: 'task-xxx',
 *     socketPath: socketDiscovery.getTaskSocketPath('task-xxx'),
 *     command: 'claude',
 *     args: ['--print', '--output-format', 'stream-json', prompt],
 *   });
 *   await server.start();
 *
 *   // Client side (in CLI attach command)
 *   const client = new AttachClient({
 *     socketPath: socketDiscovery.getTaskSocketPath('task-xxx'),
 *   });
 *   await client.connect();
 */

const AttachServer = require('./attach-server');
const AttachClient = require('./attach-client');
const RingBuffer = require('./ring-buffer');
const protocol = require('./protocol');
const socketDiscovery = require('./socket-discovery');

module.exports = {
  AttachServer,
  AttachClient,
  RingBuffer,
  protocol,
  socketDiscovery,
};
