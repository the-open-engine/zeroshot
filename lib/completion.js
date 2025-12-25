const omelette = require('omelette');
const fs = require('fs');
const path = require('path');

function setupCompletion() {
  const complete = omelette('zeroshot');

  complete.on('start', ({ reply }) => {
    reply(['--issue', '--text', '--config']);
  });

  complete.on('list', ({ reply }) => {
    reply([]);
  });

  complete.on('status', ({ reply }) => {
    // Complete with cluster IDs
    try {
      const clustersDir = path.join(process.env.HOME, '.zeroshot', 'clusters');
      if (fs.existsSync(clustersDir)) {
        const clusterIds = fs
          .readdirSync(clustersDir)
          .filter((f) => f.endsWith('.json'))
          .map((f) => f.replace('.json', ''));
        reply(clusterIds);
      } else {
        reply([]);
      }
    } catch {
      reply([]);
    }
  });

  complete.on('logs', ({ reply }) => {
    // Complete with cluster IDs or flags
    try {
      const clustersDir = path.join(process.env.HOME, '.zeroshot', 'clusters');
      const completions = ['--follow', '-f', '--limit', '-n'];

      if (fs.existsSync(clustersDir)) {
        const clusterIds = fs
          .readdirSync(clustersDir)
          .filter((f) => f.endsWith('.json'))
          .map((f) => f.replace('.json', ''));
        reply([...clusterIds, ...completions]);
      } else {
        reply(completions);
      }
    } catch {
      reply(['--follow', '-f', '--limit', '-n']);
    }
  });

  complete.on('stop', ({ reply }) => {
    // Complete with active cluster IDs
    try {
      const clustersDir = path.join(process.env.HOME, '.zeroshot', 'clusters');
      if (fs.existsSync(clustersDir)) {
        const clusterIds = fs
          .readdirSync(clustersDir)
          .filter((f) => f.endsWith('.json'))
          .map((f) => f.replace('.json', ''));
        reply(clusterIds);
      } else {
        reply([]);
      }
    } catch {
      reply([]);
    }
  });

  complete.on('kill', ({ reply }) => {
    // Complete with active cluster IDs
    try {
      const clustersDir = path.join(process.env.HOME, '.zeroshot', 'clusters');
      if (fs.existsSync(clustersDir)) {
        const clusterIds = fs
          .readdirSync(clustersDir)
          .filter((f) => f.endsWith('.json'))
          .map((f) => f.replace('.json', ''));
        reply(clusterIds);
      } else {
        reply([]);
      }
    } catch {
      reply([]);
    }
  });

  complete.on('export', ({ reply }) => {
    // Complete with cluster IDs
    try {
      const clustersDir = path.join(process.env.HOME, '.zeroshot', 'clusters');
      if (fs.existsSync(clustersDir)) {
        const clusterIds = fs
          .readdirSync(clustersDir)
          .filter((f) => f.endsWith('.json'))
          .map((f) => f.replace('.json', ''));
        reply(clusterIds);
      } else {
        reply([]);
      }
    } catch {
      reply([]);
    }
  });

  complete.on('finish', ({ reply }) => {
    // Complete with cluster IDs and flags
    try {
      const clustersDir = path.join(process.env.HOME, '.zeroshot', 'clusters');
      const completions = ['--merge', '--pr', '--push'];

      if (fs.existsSync(clustersDir)) {
        const clusterIds = fs
          .readdirSync(clustersDir)
          .filter((f) => f.endsWith('.json'))
          .map((f) => f.replace('.json', ''));
        reply([...clusterIds, ...completions]);
      } else {
        reply(completions);
      }
    } catch {
      reply(['--merge', '--pr', '--push']);
    }
  });

  complete.on('resume', ({ reply }) => {
    // Complete with cluster IDs
    try {
      const clustersDir = path.join(process.env.HOME, '.zeroshot', 'clusters');
      if (fs.existsSync(clustersDir)) {
        const clusterIds = fs
          .readdirSync(clustersDir)
          .filter((f) => f.endsWith('.json'))
          .map((f) => f.replace('.json', ''));
        reply(clusterIds);
      } else {
        reply([]);
      }
    } catch {
      reply([]);
    }
  });

  complete.on('ui', ({ reply }) => {
    reply(['--port']);
  });

  complete.on('watch', ({ reply }) => {
    reply(['--filter', '--refresh-rate', 'running', 'stopped', 'all']);
  });

  // Default completion - show commands
  complete.on('', ({ reply }) => {
    reply([
      'start',
      'list',
      'status',
      'logs',
      'stop',
      'kill',
      'finish',
      'resume',
      'export',
      'ui',
      'watch',
    ]);
  });

  complete.init();
}

module.exports = { setupCompletion };
