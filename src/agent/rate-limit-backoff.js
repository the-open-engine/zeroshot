/**
 * Rate-limit-aware backoff for API retries
 *
 * Rate limit errors (429, capacity exhausted, quota exceeded) need LONGER delays
 * than transient errors (timeouts, network issues).
 *
 * - Regular errors: 2s base, exponential backoff up to 30s
 * - Rate limits: 30s base, exponential backoff up to 5 minutes
 * - Retry-After header: Honored if present (capped at 5 min)
 */

/**
 * Check if error is a rate limit error
 * @param {Error|string} error - Error object or message
 * @returns {boolean} True if this is a rate limit error
 */
function isRateLimitError(error) {
  const msg = error?.message || String(error);
  return /\b429\b|rate.?limit|too many requests|no capacity|quota.?exceeded|resource.?exhausted/i.test(
    msg
  );
}

/**
 * Parse Retry-After from error message
 * Looks for patterns like "Retry-After: 120" or "retry after 120 seconds"
 * @param {Error} error - Error object
 * @returns {number|null} Seconds to wait, or null if not found
 */
function parseRetryAfter(error) {
  const msg = error?.message || '';
  // Match "Retry-After: 120" or "retry after 120" (seconds)
  const match = msg.match(/retry.?after[:\s]+(\d+)/i);
  return match ? parseInt(match[1], 10) : null;
}

/**
 * Calculate delay for retry with rate-limit awareness
 *
 * Rate limit errors get 30s base delay instead of 2s.
 * Regular errors use exponential backoff from 2s base.
 *
 * @param {Error} error - The error that occurred
 * @param {number} attempt - Current attempt number (1-based)
 * @param {Object} settings - Settings with backoff config
 * @param {number} [settings.backoffBaseMs=2000] - Base delay for regular errors
 * @param {number} [settings.backoffMaxMs=30000] - Max delay for regular errors
 * @param {number} [settings.jitterFactor=0.2] - Jitter factor (±20%)
 * @returns {number} Delay in milliseconds
 */
function calculateRateLimitDelay(error, attempt, settings = {}) {
  const baseDelay = settings.backoffBaseMs ?? 2000;
  const maxDelay = settings.backoffMaxMs ?? 30000;
  const jitter = settings.jitterFactor ?? 0.2;

  // Check for Retry-After header in error message
  const retryAfter = parseRetryAfter(error);
  if (retryAfter) {
    // Honor Retry-After but cap at 5 minutes
    return Math.min(retryAfter * 1000, 300000);
  }

  // Rate limits get 30s base, others get normal base
  const isRateLimit = isRateLimitError(error);
  const effectiveBase = isRateLimit ? 30000 : baseDelay;

  // Exponential: base * 2^(attempt-1)
  let delay = effectiveBase * Math.pow(2, attempt - 1);

  // Cap at appropriate max (5 min for rate limits, settings max for others)
  delay = Math.min(delay, isRateLimit ? 300000 : maxDelay);

  // Add jitter (±jitterFactor)
  const jitterAmount = delay * jitter * (Math.random() * 2 - 1);
  return Math.round(delay + jitterAmount);
}

module.exports = {
  calculateRateLimitDelay,
  isRateLimitError,
  parseRetryAfter,
};
