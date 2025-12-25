/**
 * Formatting utilities for TUI display
 */

/**
 * Format timestamp as human-readable relative time
 * @param {number} ms - Milliseconds
 * @returns {string} Formatted time (e.g., "2m 30s", "1h 15m")
 */
function formatTimestamp(ms) {
  if (!ms || ms < 0) return '-';

  const seconds = Math.floor(ms / 1000);
  const minutes = Math.floor(seconds / 60);
  const hours = Math.floor(minutes / 60);
  const days = Math.floor(hours / 24);

  if (days > 0) return `${days}d ${hours % 24}h`;
  if (hours > 0) return `${hours}h ${minutes % 60}m`;
  if (minutes > 0) return `${minutes}m ${seconds % 60}s`;
  return `${seconds}s`;
}

/**
 * Format bytes as human-readable size
 * @param {number} bytes - Bytes
 * @returns {string} Formatted size (e.g., "1.5 MB", "512 KB")
 */
function formatBytes(bytes) {
  if (!bytes || bytes === 0) return '0 B';
  if (bytes < 0) return '-';

  const units = ['B', 'KB', 'MB', 'GB'];
  let size = bytes;
  let unitIndex = 0;

  while (size >= 1024 && unitIndex < units.length - 1) {
    size /= 1024;
    unitIndex++;
  }

  return `${size.toFixed(1)} ${units[unitIndex]}`;
}

/**
 * Format CPU percentage
 * @param {number} cpu - CPU percentage (0-100)
 * @returns {string} Formatted CPU (e.g., "23.5%", "0.1%")
 */
function formatCPU(cpu) {
  if (cpu === undefined || cpu === null || cpu < 0) return '0.0%';
  return `${cpu.toFixed(1)}%`;
}

/**
 * Get state icon and color
 * @param {string} state - Task state (pending, running, completed, failed, etc.)
 * @returns {string} Colored icon
 */
function stateIcon(state) {
  const icons = {
    pending: '○',
    running: '●',
    completed: '✓',
    failed: '✗',
    killed: '⊗',
    unknown: '?',
  };

  const colors = {
    pending: 'gray',
    running: 'cyan',
    completed: 'green',
    failed: 'red',
    killed: 'red',
    unknown: 'gray',
  };

  const icon = icons[state] || icons.unknown;
  const color = colors[state] || colors.unknown;

  return `{${color}-fg}${icon}{/}`;
}

/**
 * Truncate string to max length with ellipsis
 * @param {string} str - String to truncate
 * @param {number} maxLen - Maximum length
 * @returns {string} Truncated string
 */
function truncate(str, maxLen) {
  if (!str) return '';
  if (str.length <= maxLen) return str;
  return str.substring(0, maxLen - 1) + '…';
}

/**
 * Parse event type from Claude JSON stream
 * @param {string} line - Raw log line
 * @returns {object|null} Parsed event with type, text, toolName, error
 */
function parseEvent(line) {
  let trimmed = line.trim();

  // Strip timestamp prefix if present: [1234567890]{...} -> {...}
  const timestampMatch = trimmed.match(/^\[(\d+)\](.*)$/);
  let timestamp = Date.now();
  if (timestampMatch) {
    timestamp = parseInt(timestampMatch[1]);
    trimmed = timestampMatch[2];
  }

  // Keep non-JSON lines as-is
  if (!trimmed.startsWith('{')) {
    return trimmed ? { type: 'raw', text: trimmed, timestamp } : null;
  }

  // Parse JSON events
  try {
    const event = JSON.parse(trimmed);

    // Text delta
    if (event.type === 'stream_event' && event.event?.type === 'content_block_delta') {
      return {
        type: 'text',
        text: event.event?.delta?.text || '',
        timestamp,
      };
    }
    // Tool use
    else if (event.type === 'stream_event' && event.event?.type === 'content_block_start') {
      const block = event.event?.content_block;
      if (block?.type === 'tool_use' && block?.name) {
        return {
          type: 'tool',
          toolName: block.name,
          timestamp,
        };
      }
    }
    // Assistant message
    else if (event.type === 'assistant' && event.message?.content) {
      let text = '';
      for (const content of event.message.content) {
        if (content.type === 'text') {
          text += content.text;
        }
      }
      return text ? { type: 'text', text, timestamp } : null;
    }
    // Error
    else if (event.type === 'result' && event.is_error) {
      return {
        type: 'error',
        text: event.result || 'Unknown error',
        timestamp,
      };
    }
  } catch {
    // Parse error - skip
  }

  return null;
}

export { formatTimestamp, formatBytes, formatCPU, stateIcon, truncate, parseEvent };
