'use strict';

const crypto = require('crypto');
const zlib = require('zlib');

function archiveName(version, target) {
  return `zeroshot-v${version}-${target}.tar.gz`;
}

function targetForHost(declarations, platform, arch) {
  const found = declarations.find(
    (candidate) => candidate.platform === platform && candidate.arch === arch
  );
  if (!found) {
    throw new Error(
      `UNSUPPORTED_ZEROSHOT_HOST: no prebuilt binary for ${platform}/${arch}; supported hosts: ${declarations
        .map((candidate) => `${candidate.platform}/${candidate.arch}`)
        .join(', ')}`
    );
  }
  return found;
}

function parseChecksumManifest(text) {
  const checksums = new Map();
  for (const line of text.split(/\r?\n/)) {
    if (!line) continue;
    const match = /^([0-9a-f]{64}) {2}([^/\\]+)$/.exec(line);
    if (!match) throw new Error(`invalid SHA256SUMS line: ${line}`);
    if (checksums.has(match[2])) throw new Error(`duplicate SHA256SUMS entry: ${match[2]}`);
    checksums.set(match[2], match[1]);
  }
  return checksums;
}

function sha256(contents) {
  return crypto.createHash('sha256').update(contents).digest('hex');
}

function verifyChecksum(filename, contents, manifest) {
  const checksums = manifest instanceof Map ? manifest : parseChecksumManifest(manifest);
  const expected = checksums.get(filename);
  if (!expected) throw new Error(`CHECKSUM_MISSING: SHA256SUMS has no entry for ${filename}`);
  const actual = sha256(contents);
  if (actual !== expected) {
    throw new Error(`CHECKSUM_MISMATCH: ${filename} expected ${expected} but received ${actual}`);
  }
  return true;
}

function verifyArchive(filename, archive, manifestText) {
  return verifyChecksum(filename, archive, manifestText);
}

function parseTarSize(header) {
  const value = header.toString('ascii').replace(/\0.*$/, '').trim();
  if (!/^[0-7]+$/.test(value)) throw new Error('invalid tar entry size');
  return Number.parseInt(value, 8);
}

function extractExecutable(archive, expectedName) {
  let tar;
  try {
    tar = zlib.gunzipSync(archive);
  } catch (error) {
    throw new Error(`ARCHIVE_INVALID: cannot decompress release archive: ${error.message}`);
  }
  let offset = 0;
  let executable = null;
  while (offset + 512 <= tar.length) {
    const header = tar.subarray(offset, offset + 512);
    if (header.every((byte) => byte === 0)) {
      if (!tar.subarray(offset).every((byte) => byte === 0)) {
        throw new Error('ARCHIVE_INVALID: unexpected data after tar terminator');
      }
      break;
    }
    const name = header.subarray(0, 100).toString('utf8').replace(/\0.*$/, '');
    const size = parseTarSize(header.subarray(124, 136));
    const start = offset + 512;
    const end = start + size;
    if (end > tar.length) throw new Error('ARCHIVE_INVALID: truncated tar entry');
    if (name === expectedName) {
      if (executable) throw new Error(`ARCHIVE_INVALID: duplicate ${expectedName}`);
      executable = Buffer.from(tar.subarray(start, end));
    } else {
      throw new Error(`ARCHIVE_INVALID: unexpected archive entry ${name}`);
    }
    offset = start + Math.ceil(size / 512) * 512;
  }
  if (!executable) throw new Error(`ARCHIVE_INVALID: archive does not contain ${expectedName}`);
  return executable;
}

module.exports = {
  archiveName,
  extractExecutable,
  parseChecksumManifest,
  sha256,
  targetForHost,
  verifyArchive,
  verifyChecksum,
};
