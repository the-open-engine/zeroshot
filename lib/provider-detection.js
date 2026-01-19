const { execSync, spawnSync } = require('child_process');
const fs = require('fs');
const path = require('path');

function commandExists(command) {
  if (!command) return false;
  if (command.includes(path.sep)) {
    return fs.existsSync(command);
  }
  try {
    execSync(`command -v ${command}`, { stdio: 'pipe' });
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
    const output = execSync(`command -v ${command}`, { encoding: 'utf8', stdio: 'pipe' });
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
};
