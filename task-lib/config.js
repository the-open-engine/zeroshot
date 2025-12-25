import { homedir } from 'os';
import { join } from 'path';

export const TASKS_DIR = join(homedir(), '.claude-zeroshot');
export const TASKS_FILE = join(TASKS_DIR, 'tasks.json');
export const LOGS_DIR = join(TASKS_DIR, 'logs');
export const SCHEDULES_FILE = join(TASKS_DIR, 'schedules.json');
export const SCHEDULER_PID_FILE = join(TASKS_DIR, 'scheduler.pid');
export const SCHEDULER_LOG = join(TASKS_DIR, 'scheduler.log');
export const DEFAULT_MODEL = 'sonnet';
