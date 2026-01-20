const PLATFORM_MISMATCH_REGEX =
  /EBADPLATFORM|Unsupported platform|darwin-arm64|linux-x64|@esbuild\/linux-x64/i;

function isPlatformMismatchReason(reason) {
  if (!reason) return false;
  return PLATFORM_MISMATCH_REGEX.test(String(reason));
}

function findPlatformMismatchReason(result = {}) {
  const criteriaResults = result.criteriaResults;
  if (Array.isArray(criteriaResults)) {
    for (const criteria of criteriaResults) {
      if (criteria?.status !== 'CANNOT_VALIDATE') continue;
      if (isPlatformMismatchReason(criteria.reason)) {
        return String(criteria.reason);
      }
    }
  }

  const errors = result.errors;
  if (Array.isArray(errors)) {
    for (const error of errors) {
      if (isPlatformMismatchReason(error)) {
        return String(error);
      }
    }
  }

  return null;
}

module.exports = {
  isPlatformMismatchReason,
  findPlatformMismatchReason,
};
