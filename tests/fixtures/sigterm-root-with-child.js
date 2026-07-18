const { spawn } = require('child_process');

const child = spawn(process.execPath, ['-e', 'setInterval(() => {}, 1000)'], {
  stdio: 'ignore',
});

process.stdout.write(`${child.pid}\n`);
process.on('SIGTERM', () => process.exit(0));
setInterval(() => {}, 1000);
