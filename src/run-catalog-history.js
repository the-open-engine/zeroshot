const fs = require('fs');
const { isSqliteRuntimeError, tryLoadBetterSqlite3 } = require('../lib/sqlite-runtime');

const TERMINAL_MESSAGE_TOPICS = {
  completed: 'CLUSTER_COMPLETE',
  failed: 'CLUSTER_FAILED',
  agentError: 'AGENT_ERROR',
};

function safeJsonParse(text) {
  if (typeof text !== 'string' || text.length === 0) {
    return null;
  }
  try {
    return JSON.parse(text);
  } catch {
    return null;
  }
}

function clip(text, maxLength = 96) {
  const compact = String(text || '')
    .replace(/\s+/g, ' ')
    .trim();
  if (compact.length <= maxLength) {
    return compact;
  }
  return `${compact.slice(0, maxLength - 3)}...`;
}

function extractIssueTitle(text) {
  const lines = String(text || '')
    .split(/\r?\n/)
    .map((line) => line.trim());
  const titleIndex = lines.findIndex((line) => /^##\s+Title\b/i.test(line));
  if (titleIndex >= 0) {
    const title = lines.slice(titleIndex + 1).find((line) => line.length > 0);
    if (title) {
      return title;
    }
  }
  return null;
}

function summarizeTaskText(text, contentData) {
  const issueTitle =
    contentData && typeof contentData.title === 'string' ? contentData.title.trim() : '';
  if (issueTitle.length > 0) {
    return clip(issueTitle);
  }

  const extractedIssueTitle = extractIssueTitle(text);
  if (extractedIssueTitle) {
    return clip(extractedIssueTitle);
  }

  const lines = String(text || '')
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean)
    .filter((line) => !/^#+\s/.test(line));
  return lines.length > 0 ? clip(lines[0]) : null;
}

function parsePositiveInteger(value) {
  if (typeof value === 'number' && Number.isInteger(value) && value > 0) {
    return value;
  }
  if (typeof value === 'string') {
    const parsed = Number.parseInt(value, 10);
    if (Number.isInteger(parsed) && parsed > 0) {
      return parsed;
    }
  }
  return undefined;
}

function deriveSetupFailure(text) {
  const lines = String(text || '')
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);
  if (lines.length === 0) {
    return { state: 'initializing', failureReason: null };
  }

  const interestingLine =
    [...lines]
      .reverse()
      .find(
        (line) =>
          /^Error:\s+Command failed:/i.test(line) ||
          /^npm error\b/i.test(line) ||
          /^fatal:/i.test(line) ||
          /No such file or directory/i.test(line) ||
          /not found$/i.test(line)
      ) || null;

  if (!interestingLine) {
    return { state: 'initializing', failureReason: null };
  }

  return {
    state: 'setup_failed',
    failureReason: clip(interestingLine, 180),
  };
}

function prepareHistoryQueries(db) {
  return {
    countRow: db.prepare('SELECT COUNT(*) AS count FROM messages WHERE cluster_id = ?'),
    issueOpened: db.prepare(
      `SELECT timestamp, content_text, content_data
       FROM messages
       WHERE cluster_id = ? AND topic = 'ISSUE_OPENED'
       ORDER BY timestamp ASC
       LIMIT 1`
    ),
    firstMessage: db.prepare(
      `SELECT timestamp
       FROM messages
       WHERE cluster_id = ?
       ORDER BY timestamp ASC
       LIMIT 1`
    ),
    lastMessage: db.prepare(
      `SELECT timestamp, topic, sender, content_text, content_data
       FROM messages
       WHERE cluster_id = ?
       ORDER BY timestamp DESC
       LIMIT 1`
    ),
    hasTerminalTopic: db.prepare(
      `SELECT 1
       FROM messages
       WHERE cluster_id = ? AND topic = ?
       LIMIT 1`
    ),
    failureMessage: db.prepare(
      `SELECT content_text
       FROM messages
       WHERE cluster_id = ? AND topic IN (?, ?)
       ORDER BY timestamp DESC
       LIMIT 1`
    ),
  };
}

function readMessageCount(clusterId, queries) {
  return Number(queries.countRow.get(clusterId)?.count || 0);
}

function readIssueDetails(clusterId, queries) {
  const issueOpened = queries.issueOpened.get(clusterId);
  const issueData = safeJsonParse(issueOpened?.content_data || '');
  return {
    createdAt: queries.firstMessage.get(clusterId)?.timestamp ?? issueOpened?.timestamp ?? null,
    issue: parsePositiveInteger(issueData?.issue_number),
    taskSummary: summarizeTaskText(issueOpened?.content_text || '', issueData),
  };
}

function readLastMessageDetails(clusterId, queries) {
  const lastMessage = queries.lastMessage.get(clusterId);
  return {
    lastActivityAt: lastMessage?.timestamp ?? null,
    lastTopic: lastMessage?.topic ?? null,
    lastSender: lastMessage?.sender ?? null,
    lastContentData: safeJsonParse(lastMessage?.content_data || ''),
  };
}

function readTerminalState(clusterId, queries) {
  const failureMessage = queries.failureMessage.get(
    clusterId,
    TERMINAL_MESSAGE_TOPICS.failed,
    TERMINAL_MESSAGE_TOPICS.agentError
  );
  return {
    hasComplete: Boolean(
      queries.hasTerminalTopic.get(clusterId, TERMINAL_MESSAGE_TOPICS.completed)
    ),
    hasClusterFailed: Boolean(
      queries.hasTerminalTopic.get(clusterId, TERMINAL_MESSAGE_TOPICS.failed)
    ),
    hasAgentError: Boolean(
      queries.hasTerminalTopic.get(clusterId, TERMINAL_MESSAGE_TOPICS.agentError)
    ),
    dbFailureReason: failureMessage?.content_text ? clip(failureMessage.content_text, 180) : null,
  };
}

function buildHistorySummary(clusterId, queries) {
  return {
    messageCount: readMessageCount(clusterId, queries),
    ...readIssueDetails(clusterId, queries),
    ...readLastMessageDetails(clusterId, queries),
    ...readTerminalState(clusterId, queries),
  };
}

function buildSqliteUnavailableHistory(error) {
  return {
    sqliteUnavailable: true,
    sqliteWarning: error.message,
    messageCount: null,
  };
}

function readDbHistory(clusterId, dbPath, options = {}) {
  if (!fs.existsSync(dbPath)) {
    return null;
  }

  const loadSqlite = options.loadSqlite || tryLoadBetterSqlite3;
  const { Database, error: loadError } = loadSqlite('read-only run history');
  if (!Database) {
    return buildSqliteUnavailableHistory(loadError);
  }

  let db;
  try {
    db = new Database(dbPath, { readonly: true, timeout: 5000 });
    const hasMessagesTable = db
      .prepare("SELECT 1 AS present FROM sqlite_master WHERE type = 'table' AND name = 'messages'")
      .get();
    if (!hasMessagesTable) {
      return null;
    }

    const queries = prepareHistoryQueries(db);
    const messageCount = Number(queries.countRow.get(clusterId)?.count || 0);
    if (messageCount === 0) {
      return { messageCount: 0 };
    }

    return buildHistorySummary(clusterId, queries);
  } catch (error) {
    if (isSqliteRuntimeError(error)) {
      return buildSqliteUnavailableHistory(error);
    }
    throw error;
  } finally {
    if (db) {
      try {
        db.close();
      } catch {
        // Ignore close errors - database may already be closed
      }
    }
  }
}

module.exports = {
  deriveSetupFailure,
  readDbHistory,
};
