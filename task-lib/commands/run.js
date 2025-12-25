import chalk from 'chalk';
import { spawnTask } from '../runner.js';

export function runTask(prompt, options = {}) {
  if (!prompt || prompt.trim().length === 0) {
    console.log(chalk.red('Error: Prompt is required'));
    process.exit(1);
  }

  const outputFormat = options.outputFormat || 'stream-json';
  const jsonSchema = options.jsonSchema;
  const silentJsonOutput = options.silentJsonOutput || false;

  console.log(chalk.dim('Spawning Claude task...'));
  if (options.model) {
    console.log(chalk.dim(`  Model: ${options.model}`));
  }
  if (jsonSchema && outputFormat === 'json') {
    console.log(chalk.dim(`  JSON Schema: enforced`));
    if (silentJsonOutput) {
      console.log(chalk.dim(`  Silent mode: log contains ONLY final JSON`));
    }
  }

  const task = spawnTask(prompt, {
    cwd: options.cwd || process.cwd(),
    model: options.model,
    resume: options.resume,
    continue: options.continue,
    outputFormat,
    jsonSchema,
    silentJsonOutput,
  });

  console.log(chalk.green(`\n✓ Task spawned: ${chalk.cyan(task.id)}`));
  console.log(chalk.dim(`  Log: ${task.logFile}`));
  console.log(chalk.dim(`  CWD: ${task.cwd}`));

  console.log(chalk.dim('\nCommands:'));
  console.log(chalk.dim(`  zeroshot attach ${task.id}    # Attach to task (Ctrl+B d to detach)`));
  console.log(chalk.dim(`  zeroshot logs ${task.id}      # View output`));
  console.log(chalk.dim(`  zeroshot logs -f ${task.id}   # Follow output`));
  console.log(chalk.dim(`  zeroshot status ${task.id}    # Check status`));
  console.log(chalk.dim(`  zeroshot kill ${task.id}      # Stop task`));
  console.log();

  return task;
}
