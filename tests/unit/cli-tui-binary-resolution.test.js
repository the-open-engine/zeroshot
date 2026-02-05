/**
 * Test: CLI TUI Binary Resolution
 *
 * Verifies local Rust builds are preferred over installed libexec binaries.
 */

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const { resolveRustTuiBinary } = require('../../lib/tui-launcher');

const DEFAULT_RUST_BIN_NAME = process.platform === 'win32' ? 'zeroshot-tui.exe' : 'zeroshot-tui';
const DEBUG_SUFFIX = path.join('tui-rs', 'target', 'debug', DEFAULT_RUST_BIN_NAME);
const RELEASE_SUFFIX = path.join('tui-rs', 'target', 'release', DEFAULT_RUST_BIN_NAME);
const LIBEXEC_SUFFIX = path.join('libexec', DEFAULT_RUST_BIN_NAME);

describe('CLI TUI binary resolution', function () {
  function withPatchedExistsSync(mock, callback) {
    const originalExistsSync = fs.existsSync;
    fs.existsSync = mock;
    try {
      callback();
    } finally {
      fs.existsSync = originalExistsSync;
    }
  }

  function withCleanBinaryEnv(callback) {
    const keys = ['ZEROSHOT_TUI_BINARY_PATH', 'ZEROSHOT_TUI_PATH', 'ZEROSHOT_TUI_BIN'];
    const previous = Object.fromEntries(keys.map((key) => [key, process.env[key]]));

    for (const key of keys) {
      delete process.env[key];
    }

    try {
      callback();
    } finally {
      for (const key of keys) {
        if (previous[key] === undefined) {
          delete process.env[key];
        } else {
          process.env[key] = previous[key];
        }
      }
    }
  }

  it('prefers local debug build over release and installed libexec binaries', function () {
    withCleanBinaryEnv(() => {
      withPatchedExistsSync(
        (candidate) => {
          if (candidate.endsWith(DEBUG_SUFFIX)) {
            return true;
          }
          if (candidate.endsWith(RELEASE_SUFFIX)) {
            return true;
          }
          if (candidate.endsWith(LIBEXEC_SUFFIX)) {
            return true;
          }
          return false;
        },
        () => {
          const resolved = resolveRustTuiBinary();
          assert(resolved.endsWith(DEBUG_SUFFIX));
        }
      );
    });
  });

  it('falls back to local release build when debug build is unavailable', function () {
    withCleanBinaryEnv(() => {
      withPatchedExistsSync(
        (candidate) => {
          if (candidate.endsWith(RELEASE_SUFFIX)) {
            return true;
          }
          if (candidate.endsWith(LIBEXEC_SUFFIX)) {
            return true;
          }
          return false;
        },
        () => {
          const resolved = resolveRustTuiBinary();
          assert(resolved.endsWith(RELEASE_SUFFIX));
        }
      );
    });
  });

  it('falls back to installed libexec binary when local build is unavailable', function () {
    withCleanBinaryEnv(() => {
      withPatchedExistsSync(
        (candidate) => candidate.endsWith(LIBEXEC_SUFFIX),
        () => {
          const resolved = resolveRustTuiBinary();
          assert(resolved.endsWith(LIBEXEC_SUFFIX));
        }
      );
    });
  });
});
