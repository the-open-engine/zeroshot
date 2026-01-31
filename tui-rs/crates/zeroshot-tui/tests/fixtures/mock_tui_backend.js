const { stdin, stdout } = process;

let buffer = Buffer.alloc(0);

stdin.on('data', (chunk) => {
  buffer = Buffer.concat([buffer, chunk]);
  processBuffer();
});

stdin.on('end', () => process.exit(0));

function processBuffer() {
  while (true) {
    const headerEnd = buffer.indexOf("\r\n\r\n");
    if (headerEnd === -1) {
      return;
    }
    const header = buffer.slice(0, headerEnd).toString('utf8');
    const length = parseContentLength(header);
    if (length === null) {
      process.exit(2);
    }
    const payloadStart = headerEnd + 4;
    const payloadEnd = payloadStart + length;
    if (buffer.length < payloadEnd) {
      return;
    }
    const payload = buffer.slice(payloadStart, payloadEnd).toString('utf8');
    buffer = buffer.slice(payloadEnd);
    handleMessage(JSON.parse(payload));
  }
}

function parseContentLength(header) {
  const lines = header.split("\r\n");
  for (const line of lines) {
    const parts = line.split(':');
    if (parts.length < 2) continue;
    if (parts[0].trim().toLowerCase() === 'content-length') {
      return parseInt(parts.slice(1).join(':').trim(), 10);
    }
  }
  return null;
}

function sendMessage(msg) {
  const payload = Buffer.from(JSON.stringify(msg), 'utf8');
  stdout.write(`Content-Length: ${payload.length}\r\n\r\n`);
  stdout.write(payload);
}

function handleMessage(msg) {
  if (msg.method === 'initialize') {
    sendMessage({
      jsonrpc: '2.0',
      id: msg.id,
      result: {
        protocolVersion: 1,
        server: { name: 'mock-backend', version: '0.0.0' },
        capabilities: {
          methods: [
            'initialize',
            'listClusters',
            'getClusterSummary',
            'subscribeClusterLogs',
            'subscribeClusterTimeline',
            'startClusterFromText',
            'startClusterFromIssue',
            'sendGuidanceToAgent',
            'sendGuidanceToCluster'
          ],
          notifications: ['clusterLogLines', 'clusterTimelineEvents']
        }
      }
    });

    setTimeout(() => {
      sendMessage({
        jsonrpc: '2.0',
        method: 'clusterLogLines',
        params: {
          subscriptionId: 'sub-logs-1',
          clusterId: 'cluster-1',
          lines: [],
          droppedCount: 0
        }
      });
    }, 5);
    return;
  }

  if (msg.method === 'listClusters') {
    sendMessage({
      jsonrpc: '2.0',
      id: msg.id,
      result: { clusters: [] }
    });
    return;
  }

  sendMessage({
    jsonrpc: '2.0',
    id: msg.id || 0,
    error: { code: -32601, message: 'method not found' }
  });
}
