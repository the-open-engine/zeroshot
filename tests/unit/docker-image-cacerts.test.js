/**
 * Test: Docker base image keeps CA certificates
 *
 * Regression guard for a hard-to-diagnose failure mode.
 *
 * The GitHub Copilot CLI (and other Rust-based provider CLIs) use the *system* CA certificate
 * store for HTTPS, not a bundled one. If `ca-certificates` is missing from the cluster image,
 * copilot authentication fails with a cryptic error that looks like a network problem:
 *
 *   Failed to fetch PAT user login: network fetch failed: request failed: builder error
 *   (~/.copilot/logs) No CA certificates were loaded from the system
 *
 * ...even when COPILOT_GITHUB_TOKEN is valid and general egress works (Node's own fetch keeps
 * working because Node bundles its own CAs, which makes this doubly confusing to debug).
 *
 * The base image is derived from `node:20-slim`, which does NOT ship system CA certificates,
 * so the Dockerfile must install them explicitly. This test fails loudly if that line is ever
 * removed, so the breakage is caught at review time instead of inside a container.
 */

const assert = require('assert');
const fs = require('fs');
const path = require('path');

const DOCKERFILE_PATH = path.join(
  __dirname,
  '..',
  '..',
  'docker',
  'zeroshot-cluster',
  'Dockerfile'
);

describe('Docker cluster image: CA certificates', function () {
  let dockerfile;

  before(function () {
    dockerfile = fs.readFileSync(DOCKERFILE_PATH, 'utf8');
  });

  it('installs ca-certificates (required for copilot/Rust CLI HTTPS auth)', function () {
    // Strip comment lines so a mention in a comment can never satisfy the guard.
    const instructions = dockerfile
      .split('\n')
      .filter((line) => !line.trim().startsWith('#'))
      .join('\n');

    assert.match(
      instructions,
      /\bca-certificates\b/,
      'docker/zeroshot-cluster/Dockerfile must install the `ca-certificates` package. ' +
        'Without it, the Copilot CLI (Rust) cannot load system CA certs and authentication ' +
        'fails with a misleading "network fetch failed: builder error". Do not remove it.'
    );
  });

  it('derives from node:20-slim, which is why ca-certificates must be explicit', function () {
    // If the base image ever changes to one that bundles system CA certs, this test documents
    // the assumption behind the guard above (slim images ship no system CA store).
    assert.match(
      dockerfile,
      /^FROM\s+node:\d+-slim/m,
      'Base image is expected to be a slim node image (no system CA certs by default).'
    );
  });
});
