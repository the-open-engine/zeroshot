/**
 * Selection policies for the lesson library (§4.2). A policy is ANY object:
 *
 *   {
 *     name: string,
 *     version: integer,
 *     sampleSelection(candidates, limit, rng) -> [{ index, score }]
 *   }
 *
 * `candidates` carries the Beta-Bernoulli sufficient statistics
 * [{ lesson_id, alpha, beta }] — Bernoulli-family policies use them, other
 * policies (deterministic, externally scored, voting-based, ...) may ignore
 * them. `sampleSelection` returns the chosen candidate indices in selection
 * order, each with an OPTIONAL policy-defined score (for Thompson-Beta it is
 * the theta draw; deterministic policies may return null).
 *
 * The store wraps every policy with Monte-Carlo propensity estimation (it
 * replicates the policy's OWN sampler and tallies inclusion), so any policy
 * satisfying this interface automatically satisfies the decision log's
 * propensity contract: swapping selection algorithms is a policy change,
 * never a store change. Each logged decision records the policy id
 * (name@version) as the logging policy of record — the provenance
 * off-policy evaluation (§5.3 ratio lift) cannot live without.
 */

// Box-Muller standard normal from an injectable uniform rng.
function sampleStandardNormal(rng) {
  let u = 0;
  while (u === 0) u = rng(); // guard log(0)
  return Math.sqrt(-2 * Math.log(u)) * Math.cos(2 * Math.PI * rng());
}

// Marsaglia-Tsang gamma sampler. Local implementation, no new dependencies.
function sampleGamma(shape, rng) {
  if (shape <= 0) {
    throw new Error(`sampleGamma: shape must be > 0, got ${shape}`);
  }
  if (shape < 1) {
    // Boost: Gamma(k) = Gamma(k + 1) * U^(1/k)
    return sampleGamma(shape + 1, rng) * Math.pow(rng(), 1 / shape);
  }
  const d = shape - 1 / 3;
  const c = 1 / Math.sqrt(9 * d);
  for (;;) {
    let x;
    let v;
    do {
      x = sampleStandardNormal(rng);
      v = 1 + c * x;
    } while (v <= 0);
    v = v * v * v;
    const u = rng();
    if (u <= 0) continue;
    if (u < 1 - 0.0331 * x * x * x * x) return d * v;
    if (Math.log(u) < 0.5 * x * x + d * (1 - v + Math.log(v))) return d * v;
  }
}

// §4.2 Thompson sampling over per-lesson Beta(helpful+1, harmful+1)
// posteriors: draw one theta per candidate, keep the top `limit`.
// NOTE: consumes rng in candidate order (alpha gamma, then beta gamma) —
// LessonStore selectLessons/selectWithDecision preserve this exact order so
// seeded draws are reproducible across refactors.
const THOMPSON_BETA_POLICY = {
  name: 'thompson-beta',
  version: 1,
  sampleSelection(candidates, limit, rng = Math.random) {
    const scored = candidates.map((candidate, index) => {
      const g1 = sampleGamma(candidate.alpha, rng);
      const g2 = sampleGamma(candidate.beta, rng);
      return { index, score: g1 / (g1 + g2) };
    });
    scored.sort((a, b) => b.score - a.score);
    return scored.slice(0, Math.max(0, limit));
  },
};

function policyId(policy) {
  return `${policy.name}@${policy.version}`;
}

const DEFAULT_POLICY = THOMPSON_BETA_POLICY;
const DEFAULT_POLICY_ID = policyId(DEFAULT_POLICY);

// String-addressable policies ('name@version'); object policies can always
// be injected directly without registration.
const POLICY_REGISTRY = new Map([[DEFAULT_POLICY_ID, DEFAULT_POLICY]]);

// Accepts a policy object, a registry id string, or null (default policy).
function resolvePolicy(ref) {
  if (!ref) {
    return DEFAULT_POLICY;
  }
  if (typeof ref.sampleSelection === 'function') {
    return ref;
  }
  const policy = POLICY_REGISTRY.get(String(ref));
  if (!policy) {
    throw new Error(`unknown selection policy: ${ref}`);
  }
  return policy;
}

module.exports = {
  THOMPSON_BETA_POLICY,
  DEFAULT_POLICY,
  DEFAULT_POLICY_ID,
  POLICY_REGISTRY,
  policyId,
  resolvePolicy,
  sampleGamma,
};
