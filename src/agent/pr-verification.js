/**
 * PR Verification - GitHub PR verification for git-pusher hook
 */

const { execSync } = require('../lib/safe-exec');

function parsePositiveInt(value, fallback) {
  const parsed = Number.parseInt(value, 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
}

function normalizePrNumber(value) {
  if (value === null || value === undefined) return null;
  const parsed = Number.parseInt(String(value), 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : null;
}

function extractPrInfoFromRawOutput(output) {
  if (!output || typeof output !== 'string') {
    return { prUrl: null, prNumber: null };
  }

  const normalizedOutput = output.replace(/\\\//g, '/');

  const prUrlRegex = /https:\/\/github\.com\/[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+\/pull\/(\d+)/g;
  const prUrlMatches = [...normalizedOutput.matchAll(prUrlRegex)];
  const lastPrUrlMatch = prUrlMatches.length > 0 ? prUrlMatches[prUrlMatches.length - 1] : null;

  const prNumberRegex = /"?pr_number"?\s*[:=]\s*"?(\d+)"?/g;
  const prNumberMatches = [...normalizedOutput.matchAll(prNumberRegex)];
  const lastPrNumberMatch =
    prNumberMatches.length > 0 ? prNumberMatches[prNumberMatches.length - 1] : null;

  return {
    prUrl: lastPrUrlMatch ? lastPrUrlMatch[0] : null,
    prNumber:
      (lastPrNumberMatch ? normalizePrNumber(lastPrNumberMatch[1]) : null) ||
      (lastPrUrlMatch ? normalizePrNumber(lastPrUrlMatch[1]) : null) ||
      null,
  };
}

async function fetchPrDataWithRetry({ cmd, cwd, prNumber, agent }) {
  const attempts = prNumber ? 6 : 1;
  const intervalMs = 5000;

  for (let attempt = 1; attempt <= attempts; attempt++) {
    try {
      return JSON.parse(execSync(cmd, { encoding: 'utf8', cwd, stdio: ['pipe', 'pipe', 'pipe'] }));
    } catch (err) {
      const isNotFound =
        err.message.includes('Could not resolve to a PullRequest') ||
        err.message.toLowerCase().includes('no pull requests found');

      if (!isNotFound) throw err;

      if (attempt < attempts) {
        agent._log(
          `⏳ PR #${prNumber} not found yet (attempt ${attempt}/${attempts}). ` +
            `GitHub API eventual consistency — retrying in ${intervalMs / 1000}s...`
        );
        await new Promise((resolve) => setTimeout(resolve, intervalMs));
        continue;
      }

      throw new Error(
        `VERIFICATION FAILED: Agent claimed PR #${prNumber || '(unknown)'} exists, ` +
          `but GitHub says it DOES NOT EXIST after ${attempts} attempts ` +
          `over ${(attempts * intervalMs) / 1000}s. Agent HALLUCINATED.`
      );
    }
  }
}

function resolvePrClaimsFromOutput({ output, providerName }) {
  const { extractJsonFromOutput } = require('./output-extraction');
  const structuredOutput = extractJsonFromOutput(output, providerName) || {};

  let claimedPrUrl = structuredOutput.pr_url || null;
  let claimedPrNumber = normalizePrNumber(structuredOutput.pr_number);

  const fallbackPrInfo = extractPrInfoFromRawOutput(output);
  if (!claimedPrUrl && fallbackPrInfo.prUrl) claimedPrUrl = fallbackPrInfo.prUrl;
  if (!claimedPrNumber && fallbackPrInfo.prNumber) claimedPrNumber = fallbackPrInfo.prNumber;

  return {
    structuredOutput,
    claimedPrUrl,
    claimedPrNumber,
    usedFallbackExtraction:
      (fallbackPrInfo.prUrl && !structuredOutput.pr_url) ||
      (fallbackPrInfo.prNumber && !structuredOutput.pr_number),
  };
}

function publishClusterComplete(agent, data) {
  agent._publish({ topic: 'CLUSTER_COMPLETE', content: { data } });
}

async function pollForMerge({ prData, agent, cwd }) {
  const pollAttempts = parsePositiveInt(process.env.ZEROSHOT_PR_MERGE_POLL_ATTEMPTS, 12);
  const pollIntervalMs = parsePositiveInt(process.env.ZEROSHOT_PR_MERGE_POLL_INTERVAL_MS, 5000);
  const prNumber = prData.number;
  const pollCmd = `gh pr view ${prNumber} --json state,mergedAt,url,number`;

  agent._log(
    `⏳ PR #${prNumber} not yet showing as merged (state="${prData.state}"). ` +
      `Polling (up to ${pollAttempts} attempts, ${pollIntervalMs / 1000}s apart)...`
  );

  let latest = prData;
  for (let attempt = 1; attempt <= pollAttempts; attempt++) {
    await new Promise((resolve) => setTimeout(resolve, pollIntervalMs));
    try {
      latest = JSON.parse(
        execSync(pollCmd, { encoding: 'utf8', cwd, stdio: ['pipe', 'pipe', 'pipe'] })
      );
    } catch {
      continue;
    }
    if (latest.mergedAt) {
      agent._log(`✅ PR #${prNumber} merge confirmed on poll attempt ${attempt}`);
      return latest;
    }
    agent._log(`⏳ Poll ${attempt}/${pollAttempts}: PR #${prNumber} still state="${latest.state}"`);
  }

  return latest;
}

function handleUnmergedPr({ prData, agent }) {
  const pollAttempts = parsePositiveInt(process.env.ZEROSHOT_PR_MERGE_POLL_ATTEMPTS, 12);
  const pollIntervalMs = parsePositiveInt(process.env.ZEROSHOT_PR_MERGE_POLL_INTERVAL_MS, 5000);
  const windowSeconds = (pollAttempts * pollIntervalMs) / 1000;

  if (prData.state === 'OPEN') {
    const reason =
      'PR merge not yet visible via GitHub API after polling window; verification deferred.';
    agent._log(`⚠️  VERIFICATION PENDING: ${reason} PR #${prData.number}, state="${prData.state}"`);
    publishClusterComplete(agent, {
      reason: 'git-pusher-complete-verification-pending',
      pr_number: prData.number,
      pr_url: prData.url,
      verification_pending: true,
      verification_state: prData.state,
      verification_polls: pollAttempts,
      verification_window_seconds: windowSeconds,
      verification_message: reason,
    });
    return;
  }

  throw new Error(
    `VERIFICATION FAILED: PR #${prData.number} exists but is not merged ` +
      `(state="${prData.state}") after ${pollAttempts} polls over ${windowSeconds}s.`
  );
}

function validatePrClaims({ structuredOutput, claimedPrNumber, claimedPrUrl }) {
  if (claimedPrNumber || claimedPrUrl) return;

  const reason = structuredOutput.summary || structuredOutput.result || 'no details provided';
  const reasonStr =
    typeof reason === 'string' ? reason.slice(0, 200) : JSON.stringify(reason).slice(0, 200);
  throw new Error(
    `git-pusher completed without creating a PR (no pr_number or pr_url in output). ` +
      `Reason: ${reasonStr}`
  );
}

function buildGhPrViewCmd(claimedPrNumber) {
  const base = 'gh pr view';
  const suffix = '--json state,mergedAt,url,number';
  return claimedPrNumber ? `${base} ${claimedPrNumber} ${suffix}` : `${base} ${suffix}`;
}

function validatePrUrl({ claimedPrUrl, prData }) {
  if (claimedPrUrl && prData.url && claimedPrUrl !== prData.url) {
    throw new Error(
      `VERIFICATION FAILED: Agent claimed PR URL ${claimedPrUrl}, ` +
        `but GitHub CLI reports ${prData.url}.`
    );
  }
}

async function verifyGithubPr({ result, agent }) {
  const providerName =
    typeof agent?._resolveProvider === 'function' ? agent._resolveProvider() : 'claude';
  const claims = resolvePrClaimsFromOutput({ output: result.output, providerName });

  if (process.env.ZEROSHOT_SKIP_GH_VERIFY === '1') {
    agent._log(`✅ VERIFICATION SKIPPED (ZEROSHOT_SKIP_GH_VERIFY=1)`);
    publishClusterComplete(agent, {
      reason: 'git-pusher-complete-verified',
      pr_number: claims.claimedPrNumber,
      pr_url: claims.claimedPrUrl,
    });
    return;
  }

  validatePrClaims(claims);

  let prData = await fetchPrDataWithRetry({
    cmd: buildGhPrViewCmd(claims.claimedPrNumber),
    cwd: agent.workingDirectory,
    prNumber: claims.claimedPrNumber,
    agent,
  });

  validatePrUrl({ claimedPrUrl: claims.claimedPrUrl, prData });

  if (claims.usedFallbackExtraction) {
    agent._log(
      `⚠️  PR metadata recovered from raw output fallback ` +
        `(provider=${providerName}, pr=#${prData.number})`
    );
  }

  if (!prData.mergedAt) {
    prData = await pollForMerge({ prData, agent, cwd: agent.workingDirectory });
  }

  if (!prData.mergedAt) {
    handleUnmergedPr({ prData, agent });
    return;
  }

  agent._log(`✅ VERIFICATION PASSED: PR #${prData.number} actually merged`);
  publishClusterComplete(agent, {
    reason: 'git-pusher-complete-verified',
    pr_number: prData.number,
    pr_url: prData.url,
  });
}

module.exports = {
  parsePositiveInt,
  normalizePrNumber,
  extractPrInfoFromRawOutput,
  fetchPrDataWithRetry,
  resolvePrClaimsFromOutput,
  publishClusterComplete,
  verifyGithubPr,
};
