const childProcess = require('child_process');
const { execSync, spawnSync } = childProcess;
const fs = require('fs');
const path = require('path');

function commandLookupCommand(command) {
  return process.platform === 'win32' ? `where ${command}` : `command -v ${command}`;
}

function commandExists(command) {
  if (!command) return false;
  if (command.includes(path.sep)) {
    return fs.existsSync(command);
  }
  try {
    execSync(commandLookupCommand(command), { stdio: 'pipe' });
    return true;
  } catch {
    return false;
  }
}

function getCommandPath(command) {
  if (!command) return null;
  if (command.includes(path.sep)) {
    return fs.existsSync(command) ? command : null;
  }
  try {
    const output = execSync(commandLookupCommand(command), { encoding: 'utf8', stdio: 'pipe' });
    return output.trim() || null;
  } catch {
    return null;
  }
}

function getHelpOutput(command, args = []) {
  if (!commandExists(command)) return '';

  const attempt = (flag) => {
    const result = spawnSync(command, [...args, flag], { encoding: 'utf8' });
    const output = `${result.stdout || ''}${result.stderr || ''}`;
    return output.trim();
  };

  const help = attempt('--help');
  if (help) return help;

  const alt = attempt('-h');
  return alt || '';
}

function getVersionOutput(command, args = []) {
  if (!commandExists(command)) return '';
  const result = spawnSync(command, [...args, '--version'], { encoding: 'utf8' });
  const output = `${result.stdout || ''}${result.stderr || ''}`;
  return output.trim();
}

module.exports = {
  commandExists,
  getCommandPath,
  getHelpOutput,
  getVersionOutput,
  commandLookupCommand,
};
