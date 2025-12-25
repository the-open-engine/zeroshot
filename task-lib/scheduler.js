#!/usr/bin/env node

/**
 * Scheduler daemon - runs as background process
 * Checks for due scheduled tasks and spawns them
 */

import { readFileSync, writeFileSync, existsSync, appendFileSync, unlinkSync } from 'fs';
import { loadSchedules, updateSchedule } from './store.js';
import { spawnTask } from './runner.js';
import { SCHEDULER_PID_FILE, SCHEDULER_LOG } from './config.js';

const CHECK_INTERVAL = 60000; // 60 seconds

/**
 * Parse human-readable interval to milliseconds
 * Supports: 30s, 5m, 2h, 1d, 1w
 */
export function parseInterval(str) {
  const match = str.match(/^(\d+)(s|m|h|d|w)$/i);
  if (!match) return null;

  const value = parseInt(match[1], 10);
  const unit = match[2].toLowerCase();

  const multipliers = {
    s: 1000,
    m: 60 * 1000,
    h: 60 * 60 * 1000,
    d: 24 * 60 * 60 * 1000,
    w: 7 * 24 * 60 * 60 * 1000,
  };

  return value * multipliers[unit];
}

/**
 * Parse cron expression and get next run time
 * Simple cron parser supporting: minute hour day month weekday
 */
export function getNextCronTime(cronExpr, fromDate = new Date()) {
  const parts = cronExpr.trim().split(/\s+/);
  if (parts.length !== 5) return null;

  const [minute, hour] = parts;

  // Simple implementation - just handle basic cases
  // For full cron support, use cron-parser package
  const next = new Date(fromDate);
  next.setSeconds(0);
  next.setMilliseconds(0);

  // Handle simple cases
  if (minute !== '*') {
    const mins = minute.split(',').map((m) => parseInt(m, 10));
    const currentMin = next.getMinutes();
    const nextMin = mins.find((m) => m > currentMin) ?? mins[0];
    if (nextMin <= currentMin) {
      next.setHours(next.getHours() + 1);
    }
    next.setMinutes(nextMin);
  }

  if (hour !== '*') {
    const hours = hour.split(',').map((h) => parseInt(h, 10));
    const currentHour = next.getHours();
    const nextHour = hours.find((h) => h > currentHour) ?? hours[0];
    if (nextHour <= currentHour && minute === '*') {
      next.setDate(next.getDate() + 1);
    }
    if (nextHour !== currentHour) {
      next.setMinutes(minute === '*' ? 0 : parseInt(minute, 10));
    }
    next.setHours(nextHour);
  }

  // For complex cron expressions, fall back to 1 hour from now
  // Full implementation would need cron-parser
  if (next <= fromDate) {
    next.setTime(fromDate.getTime() + 60 * 60 * 1000);
  }

  return next;
}

/**
 * Calculate next run time for a schedule
 */
export function calculateNextRun(schedule) {
  if (schedule.interval) {
    return new Date(Date.now() + schedule.interval);
  }
  if (schedule.cron) {
    return getNextCronTime(schedule.cron);
  }
  return null;
}

/**
 * Log to scheduler log file
 */
function log(msg) {
  const timestamp = new Date().toISOString();
  const line = `[${timestamp}] ${msg}\n`;
  appendFileSync(SCHEDULER_LOG, line);
}

/**
 * Check and run due schedules
 */
function checkSchedules() {
  const schedules = loadSchedules();
  const now = new Date();

  for (const schedule of Object.values(schedules)) {
    if (!schedule.enabled) continue;

    const nextRun = new Date(schedule.nextRunAt);
    if (nextRun > now) continue;

    // Schedule is due - spawn task
    log(`Running scheduled task: ${schedule.id} - "${schedule.prompt.slice(0, 50)}..."`);

    try {
      const task = spawnTask(schedule.prompt, {
        cwd: schedule.cwd,
        scheduleId: schedule.id,
      });

      // Update schedule with next run time
      const nextRunAt = calculateNextRun(schedule);
      updateSchedule(schedule.id, {
        lastRunAt: now.toISOString(),
        lastTaskId: task.id,
        nextRunAt: nextRunAt ? nextRunAt.toISOString() : null,
      });

      log(
        `Spawned task ${task.id} for schedule ${schedule.id}, next run: ${nextRunAt?.toISOString() || 'none'}`
      );
    } catch (err) {
      log(`Error spawning task for schedule ${schedule.id}: ${err.message}`);
    }
  }
}

/**
 * Start the scheduler daemon
 */
export function startDaemon() {
  // Check if already running
  if (existsSync(SCHEDULER_PID_FILE)) {
    const existingPid = parseInt(readFileSync(SCHEDULER_PID_FILE, 'utf-8').trim(), 10);
    try {
      process.kill(existingPid, 0);
      console.log(`Scheduler already running (PID: ${existingPid})`);
      return false;
    } catch {
      // Process not running, clean up stale PID file
      unlinkSync(SCHEDULER_PID_FILE);
    }
  }

  // Write PID file
  writeFileSync(SCHEDULER_PID_FILE, String(process.pid));

  log(`Scheduler daemon started (PID: ${process.pid})`);
  console.log(`Scheduler daemon started (PID: ${process.pid})`);

  // Run check loop
  const runLoop = async () => {
    while (true) {
      try {
        await checkSchedules();
      } catch (err) {
        // Log error with full stack trace - scheduler errors are critical bugs
        const errorMsg = `SCHEDULER ERROR: ${err.message}\nStack: ${err.stack}`;
        log(errorMsg);
        console.error(errorMsg);
      }
      await new Promise((r) => setTimeout(r, CHECK_INTERVAL));
    }
  };

  // Handle shutdown
  process.on('SIGTERM', () => {
    log('Scheduler daemon stopping (SIGTERM)');
    if (existsSync(SCHEDULER_PID_FILE)) {
      unlinkSync(SCHEDULER_PID_FILE);
    }
    process.exit(0);
  });

  process.on('SIGINT', () => {
    log('Scheduler daemon stopping (SIGINT)');
    if (existsSync(SCHEDULER_PID_FILE)) {
      unlinkSync(SCHEDULER_PID_FILE);
    }
    process.exit(0);
  });

  runLoop();
  return true;
}

/**
 * Stop the scheduler daemon
 */
export function stopDaemon() {
  if (!existsSync(SCHEDULER_PID_FILE)) {
    console.log('Scheduler is not running');
    return false;
  }

  const pid = parseInt(readFileSync(SCHEDULER_PID_FILE, 'utf-8').trim(), 10);

  try {
    process.kill(pid, 'SIGTERM');
    unlinkSync(SCHEDULER_PID_FILE);
    console.log(`Scheduler stopped (PID: ${pid})`);
    log(`Scheduler daemon stopped by user (PID: ${pid})`);
    return true;
  } catch {
    // Process not running, clean up
    unlinkSync(SCHEDULER_PID_FILE);
    console.log('Scheduler was not running (cleaned up stale PID file)');
    return false;
  }
}

/**
 * Get daemon status
 */
export function getDaemonStatus() {
  if (!existsSync(SCHEDULER_PID_FILE)) {
    return { running: false, pid: null };
  }

  const pid = parseInt(readFileSync(SCHEDULER_PID_FILE, 'utf-8').trim(), 10);

  try {
    process.kill(pid, 0);
    return { running: true, pid };
  } catch {
    return { running: false, pid: null, stale: true };
  }
}

// If run directly, start daemon
if (process.argv[1]?.endsWith('scheduler.js')) {
  startDaemon();
}
