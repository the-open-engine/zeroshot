'use strict';

const fs = require('node:fs/promises');

function selectedAsset(metadata, prefix, name) {
  const matches = metadata.assets?.filter((asset) => asset.name === name) ?? [];
  const asset = matches[0];
  if (
    matches.length !== 1 ||
    !asset.url?.startsWith(`${prefix}assets/`) ||
    !/^\d+$/.test(asset.url.slice(`${prefix}assets/`.length)) ||
    !Number.isSafeInteger(asset.size) ||
    asset.size < 0
  ) {
    throw new Error('GitHub release asset is missing or ambiguous');
  }
  return asset;
}

async function releaseAsset({ repository, tag, name, fetcher }, headers) {
  const prefix = `https://api.github.com/repos/${repository}/releases/`;
  const release = await fetcher(`${prefix}tags/${encodeURIComponent(tag)}`, {
    headers,
    redirect: 'error',
  });
  if (!release.ok) throw new Error(`GitHub release lookup failed (HTTP ${release.status})`);
  return selectedAsset(await release.json(), prefix, name);
}

async function assetResponse(asset, headers, fetcher) {
  let response = await fetcher(asset.url, {
    headers: { ...headers, Accept: 'application/octet-stream' },
    redirect: 'manual',
  });
  if ([301, 302, 303, 307, 308].includes(response.status)) {
    const location = new URL(response.headers.get('location'));
    if (location.protocol !== 'https:') throw new Error('GitHub asset redirect is not HTTPS');
    // Signed storage URLs authorize the download themselves; never forward the GitHub credential.
    response = await fetcher(location.href, { redirect: 'error' });
  }
  if (!response.ok) throw new Error(`GitHub asset download failed (HTTP ${response.status})`);
  return response;
}

async function downloadReleaseAsset({ repository, tag, name, output, token, fetcher = fetch }) {
  if (!/^[\w.-]+\/[\w.-]+$/.test(repository) || !tag || !name || !output) {
    throw new Error('usage: github-release-asset.js REPOSITORY TAG ASSET OUTPUT');
  }
  const headers = {
    Accept: 'application/vnd.github+json',
    'X-GitHub-Api-Version': '2022-11-28',
    'User-Agent': 'zeroshot-build',
    ...(token ? { Authorization: `Bearer ${token}` } : {}),
  };
  const asset = await releaseAsset({ repository, tag, name, fetcher }, headers);
  const response = await assetResponse(asset, headers, fetcher);
  const bytes = Buffer.from(await response.arrayBuffer());
  if (bytes.length !== asset.size) throw new Error('GitHub asset download has an unexpected size');
  await fs.writeFile(output, bytes, { flag: 'wx' });
}

if (require.main === module) {
  const [repository, tag, name, output] = process.argv.slice(2);
  downloadReleaseAsset({ repository, tag, name, output, token: process.env.GH_TOKEN }).catch(
    (error) => {
      process.stderr.write(`${error.message}\n`);
      process.exitCode = 1;
    }
  );
}

module.exports = { downloadReleaseAsset };
