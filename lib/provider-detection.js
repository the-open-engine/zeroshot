const { execSync, spawnSync } = require('child_process');
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
    return output.trim().split(/\r?\n/)[0] || null;
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

function resolveWindowsCommandSpawn(command, args = []) {
  if (process.platform !== 'win32') {
    return { command, args: [...args] };
  }

  const resolvedCommand = getCommandPath(command) || command;
  if (!/\.(cmd|bat)$/i.test(resolvedCommand)) {
    return { command: resolvedCommand, args: [...args] };
  }

  const wrapperContent = fs.readFileSync(resolvedCommand, 'utf8');
  const scriptPath = extractNodeScriptFromCmdWrapper(wrapperContent, resolvedCommand);

  if (!scriptPath) {
    return { command: resolvedCommand, args: [...args] };
  }

  return {
    command: process.execPath,
    args: [scriptPath, ...args],
  };
}

function extractNodeScriptFromCmdWrapper(wrapperContent, wrapperPath) {
  const normalized = wrapperContent.replace(/\r\n/g, '\n');
  const match = normalized.match(/"%~dp0\\([^"\n]+\.(?:js|cjs|mjs))"/i);
  if (!match) {
    return null;
  }

  const relativeScriptPath = match[1].replace(/\\/g, path.sep);
  return path.resolve(path.dirname(wrapperPath), relativeScriptPath);
}

module.exports = {
  commandExists,
  getCommandPath,
  getHelpOutput,
  getVersionOutput,
  commandLookupCommand,
  resolveWindowsCommandSpawn,
  extractNodeScriptFromCmdWrapper,
};
