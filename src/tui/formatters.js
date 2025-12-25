/**
 * TUI Display Formatters
 * Converts raw values to human-readable formats for terminal display
 */

/**
 * Convert milliseconds to human-readable uptime string
 * @param {number} ms - Milliseconds
 * @returns {string} Formatted uptime (e.g., "5m 23s", "2h 15m", "3d 4h")
 */
const formatTimestamp = (ms) => {
  if (!ms || ms < 0) return '0s';

  const seconds = Math.floor(ms / 1000);

  if (seconds < 60) {
    return `${seconds}s`;
  }

  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) {
    const remainingSeconds = seconds % 60;
    return remainingSeconds > 0 ? `${minutes}m ${remainingSeconds}s` : `${minutes}m`;
  }

  const hours = Math.floor(minutes / 60);
  if (hours < 24) {
    const remainingMinutes = minutes % 60;
    return remainingMinutes > 0 ? `${hours}h ${remainingMinutes}m` : `${hours}h`;
  }

  const days = Math.floor(hours / 24);
  const remainingHours = hours % 24;
  return remainingHours > 0 ? `${days}d ${remainingHours}h` : `${days}d`;
};

/**
 * Convert bytes to human-readable size string
 * @param {number} bytes - Number of bytes
 * @returns {string} Formatted size (e.g., "245 MB", "1.2 GB", "512 KB")
 */
const formatBytes = (bytes) => {
  if (!bytes || bytes < 0) return '0 B';

  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let size = bytes;
  let unitIndex = 0;

  while (size >= 1000 && unitIndex < units.length - 1) {
    size /= 1000;
    unitIndex++;
  }

  const formatted = size < 10 ? size.toFixed(1) : Math.round(size);
  return `${formatted} ${units[unitIndex]}`;
};

/**
 * Format CPU percentage with consistent precision
 * @param {number} percent - CPU percentage (0-100)
 * @returns {string} Formatted percentage (e.g., "12.3%", "0.1%")
 */
const formatCPU = (percent) => {
  if (typeof percent !== 'number' || percent < 0) return '0.0%';
  if (percent > 100) percent = 100;

  return `${percent.toFixed(1)}%`;
};

/**
 * Map cluster state to unicode icon
 * @param {string} state - Cluster state (running, stopped, initializing, stopping, failed, killed)
 * @returns {string} Unicode icon representing state
 */
const stateIcon = (state) => {
  const icons = {
    running: '●', // filled circle (green)
    stopped: '○', // hollow circle
    initializing: '◐', // half circle
    stopping: '◑', // half circle other way
    failed: '⚠', // warning
    killed: '⚠', // warning
  };

  return icons[state] || '?';
};

/**
 * Truncate string with ellipsis if exceeds max length
 * @param {string} str - String to truncate
 * @param {number} maxLen - Maximum length
 * @returns {string} Truncated string with "..." if needed
 */
const truncate = (str, maxLen) => {
  if (!str || typeof str !== 'string') return '';
  if (str.length <= maxLen) return str;

  return str.substring(0, maxLen - 3) + '...';
};

/**
 * Format duration between two timestamps
 * @param {number} startMs - Start timestamp in milliseconds
 * @param {number} endMs - End timestamp in milliseconds (null = now)
 * @returns {string} Formatted duration (e.g., "5m 23s", "2h 15m")
 */
const formatDuration = (startMs, endMs) => {
  if (!startMs || startMs < 0) return '0s';

  const end = endMs && endMs > 0 ? endMs : Date.now();
  const duration = Math.max(0, end - startMs);

  return formatTimestamp(duration);
};

module.exports = {
  formatTimestamp,
  formatBytes,
  formatCPU,
  stateIcon,
  truncate,
  formatDuration,
};
