const SQLITE_RUNTIME_ERROR_CODE = 'ZEROSHOT_SQLITE_RUNTIME_UNAVAILABLE';
const SQLITE_REBUILD_HINT =
  'Rebuild SQLite bindings with `cd external/zeroshot && npm rebuild better-sqlite3 || npm install`.';

const SQLITE_RUNTIME_PATTERNS = [
  /better-sqlite3/i,
  /node_module_version/i,
  /node\.js version/i,
  /could not locate the bindings file/i,
  /module did not self-register/i,
  /invalid elf header/i,
  /was compiled against a different node\.js version/i,
  /cannot find module .*better-sqlite3/i,
];

function isSqliteRuntimeError(error) {
  if (!error) {
    return false;
  }

  if (error.code === SQLITE_RUNTIME_ERROR_CODE) {
    return true;
  }

  const message = `${error.message || ''}\n${error.stack || ''}`;
  return SQLITE_RUNTIME_PATTERNS.some((pattern) => pattern.test(message));
}

function buildSqliteRuntimeMessage(purpose, error) {
  const detail = error?.message ? ` Root cause: ${error.message}` : '';
  return `SQLite runtime unavailable for ${purpose}. ${SQLITE_REBUILD_HINT}${detail}`;
}

function createSqliteRuntimeError(purpose, error) {
  if (error?.code === SQLITE_RUNTIME_ERROR_CODE) {
    return error;
  }

  const wrapped = new Error(buildSqliteRuntimeMessage(purpose, error));
  wrapped.code = SQLITE_RUNTIME_ERROR_CODE;
  wrapped.cause = error;
  return wrapped;
}

function loadBetterSqlite3OrThrow(purpose) {
  try {
    return require('better-sqlite3');
  } catch (error) {
    throw createSqliteRuntimeError(purpose, error);
  }
}

function tryLoadBetterSqlite3(purpose) {
  try {
    return { Database: require('better-sqlite3'), error: null };
  } catch (error) {
    return { Database: null, error: createSqliteRuntimeError(purpose, error) };
  }
}

module.exports = {
  SQLITE_RUNTIME_ERROR_CODE,
  SQLITE_REBUILD_HINT,
  isSqliteRuntimeError,
  buildSqliteRuntimeMessage,
  createSqliteRuntimeError,
  loadBetterSqlite3OrThrow,
  tryLoadBetterSqlite3,
};
