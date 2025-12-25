import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'fs';
import { TASKS_DIR, TASKS_FILE, LOGS_DIR, SCHEDULES_FILE } from './config.js';
import { generateName } from './name-generator.js';
import lockfile from 'proper-lockfile';

// Lock options for sync API (no retries allowed)
const LOCK_OPTIONS = {
  stale: 30000, // Consider lock stale after 30s
};

// Retry wrapper for sync lock acquisition
function lockWithRetry(file, options, maxRetries = 100, delayMs = 100) {
  for (let i = 0; i < maxRetries; i++) {
    try {
      return lockfile.lockSync(file, options);
    } catch (err) {
      if (err.code === 'ELOCKED' && i < maxRetries - 1) {
        // File is locked, wait and retry
        const start = Date.now();
        while (Date.now() - start < delayMs) {
          // Busy wait (sync)
        }
        continue;
      }
      throw err;
    }
  }
  throw new Error(`Failed to acquire lock after ${maxRetries} retries`);
}

export function ensureDirs() {
  if (!existsSync(TASKS_DIR)) mkdirSync(TASKS_DIR, { recursive: true });
  if (!existsSync(LOGS_DIR)) mkdirSync(LOGS_DIR, { recursive: true });
}

/**
 * Read tasks.json (no locking - use for read-only operations)
 */
export function loadTasks() {
  ensureDirs();
  if (!existsSync(TASKS_FILE)) return {};
  const content = readFileSync(TASKS_FILE, 'utf-8');
  try {
    return JSON.parse(content);
  } catch (error) {
    throw new Error(
      `CRITICAL: tasks.json is corrupted and cannot be parsed. Error: ${error.message}. Content: ${content.slice(0, 200)}...`
    );
  }
}

/**
 * Write tasks.json (no locking - internal use only)
 */
export function saveTasks(tasks) {
  ensureDirs();
  writeFileSync(TASKS_FILE, JSON.stringify(tasks, null, 2));
}

/**
 * Atomic read-modify-write with file locking
 * @param {Function} modifier - Function that receives tasks object and returns modified tasks
 * @returns {any} - Return value from modifier function
 */
export function withTasksLock(modifier) {
  ensureDirs();

  // Create file if it doesn't exist (needed for locking)
  if (!existsSync(TASKS_FILE)) {
    writeFileSync(TASKS_FILE, '{}');
  }

  let release;
  try {
    // Acquire lock (blocks until available)
    release = lockWithRetry(TASKS_FILE, LOCK_OPTIONS);

    // Read current state
    const content = readFileSync(TASKS_FILE, 'utf-8');
    let tasks;
    try {
      tasks = JSON.parse(content);
    } catch (error) {
      throw new Error(`CRITICAL: tasks.json is corrupted. Error: ${error.message}`);
    }

    // Apply modification
    const result = modifier(tasks);

    // Write back
    writeFileSync(TASKS_FILE, JSON.stringify(tasks, null, 2));

    return result;
  } finally {
    if (release) {
      release();
    }
  }
}

export function getTask(id) {
  const tasks = loadTasks();
  return tasks[id];
}

export function updateTask(id, updates) {
  return withTasksLock((tasks) => {
    if (!tasks[id]) return null;
    tasks[id] = {
      ...tasks[id],
      ...updates,
      updatedAt: new Date().toISOString(),
    };
    return tasks[id];
  });
}

export function addTask(task) {
  return withTasksLock((tasks) => {
    tasks[task.id] = task;
    return task;
  });
}

export function removeTask(id) {
  withTasksLock((tasks) => {
    delete tasks[id];
  });
}

export function generateId() {
  return generateName('task');
}

export function generateScheduleId() {
  return generateName('sched');
}

// Schedule management - same pattern with locking

function withSchedulesLock(modifier) {
  ensureDirs();

  if (!existsSync(SCHEDULES_FILE)) {
    writeFileSync(SCHEDULES_FILE, '{}');
  }

  let release;
  try {
    release = lockWithRetry(SCHEDULES_FILE, LOCK_OPTIONS);

    const content = readFileSync(SCHEDULES_FILE, 'utf-8');
    let schedules;
    try {
      schedules = JSON.parse(content);
    } catch (error) {
      throw new Error(`CRITICAL: schedules.json is corrupted. Error: ${error.message}`);
    }

    const result = modifier(schedules);
    writeFileSync(SCHEDULES_FILE, JSON.stringify(schedules, null, 2));

    return result;
  } finally {
    if (release) {
      release();
    }
  }
}

export function loadSchedules() {
  ensureDirs();
  if (!existsSync(SCHEDULES_FILE)) return {};
  const content = readFileSync(SCHEDULES_FILE, 'utf-8');
  try {
    return JSON.parse(content);
  } catch (error) {
    throw new Error(
      `CRITICAL: schedules.json is corrupted and cannot be parsed. Error: ${error.message}. Content: ${content.slice(0, 200)}...`
    );
  }
}

export function saveSchedules(schedules) {
  ensureDirs();
  writeFileSync(SCHEDULES_FILE, JSON.stringify(schedules, null, 2));
}

export function getSchedule(id) {
  const schedules = loadSchedules();
  return schedules[id];
}

export function addSchedule(schedule) {
  return withSchedulesLock((schedules) => {
    schedules[schedule.id] = schedule;
    return schedule;
  });
}

export function updateSchedule(id, updates) {
  return withSchedulesLock((schedules) => {
    if (!schedules[id]) return null;
    schedules[id] = {
      ...schedules[id],
      ...updates,
      updatedAt: new Date().toISOString(),
    };
    return schedules[id];
  });
}

export function removeSchedule(id) {
  withSchedulesLock((schedules) => {
    delete schedules[id];
  });
}
