# LYO Reflector Design — Abduction, Elaboration, and the A/B Protocol

Status: **draft** (2026-07-22). v0 (`template@1`) shipped in `df4fbed` behind the
pluggable reflector interface; this document specifies `elaborator@1` and the
experiment that decides whether it replaces the default.

References the LYO design doc throughout (§2 reflector role, §4.2 selection,
§5.1 validation-grounded counters, §5.3 decision log, Appendix B.4 blast-radius
containment) — section numbers follow the code comments in `src/lyo/`.

---

## 1. The Peirce frame: what was missing

The learning loop is Peirce's inference cycle, and exactly one stage was a stub:

| Peirce                                                 | Loop component                                                                          | Status                |
| ------------------------------------------------------ | --------------------------------------------------------------------------------------- | --------------------- |
| **Abduction** — originate the hypothesis               | Reflector: rejected validation → candidate explanation + intervention                   | was a string template |
| **Deduction** — explicate the hypothesis's predictions | `lesson_decision` log + `lesson_application` rows: per-injection, per-cycle predictions | built (v0.2)          |
| **Induction** — grade the predictions experimentally   | Beta-Bernoulli counters, Wilson retention gate, ratio-lift estimator                    | built, tested         |

Two consequences:

1. **No abstraction can arise downstream.** Induction only grades what
   abduction originates. If the hypothesis generator emits raw validator
   feedback, the counters faithfully measure the value of raw validator
   feedback. This is the formal answer to "where is the compression?": it is
   the reflector's job and nobody else's.
2. **Abduction may be speculative because induction is honest.** A confabulated
   lesson produces interventions that don't help; `harmful_count` grows; the
   Wilson gate quarantines it. The reflector does not need to be right — it
   needs to be _worth testing_. (Peirce's economy of research: hypotheses must
   be proposed in an order worth the testing budget. Thompson sampling over
   lesson posteriors is that budget policy.)

## 2. What `template@1` actually measures

`template@1` (the v0 default, extracted byte-identical in `df4fbed`) sets:

- `explanation` = the validator's feedback, truncated to 500 chars
- `intervention` = `"Address the validator feedback before retrying.\n\nLatest validation:\n<same feedback>"`

Its near-perfect counter scores (e.g. 20✓/0✗ in early dogfooding) are real but
misleading: the counter measures "handing the agent its own error message
usually fixes the retry." Of course it does. The delivery loop is validated;
transferable knowledge has never existed in the store, so its value has never
been measured. Predicted signature: template lessons help only when the _same
error recurs_; they should fail to transfer across tasks. §6 makes this
testable.

## 3. The elicitation contract (SSR-derived)

From "LLMs Reproduce Human Purchase Intent via Semantic Similarity Elicitation
of Likert Ratings" (Maier et al., PyMC Labs / Colgate, arXiv:2510.08338):
direct numeric elicitation from an LLM collapses to unrealistically narrow
distributions ("models regress to 'typical' answers"); eliciting free text
first and _deriving_ the score restores realistic, reliable ratings (90% of
human test–retest reliability, KS > 0.85). Aligned judging best practices:
rationale before score, crisp rubric, low temperature, explicit bias
mitigation.

The contract for any LLM reflector:

1. **Text first.** The elaboration is the primary artifact — in LYO it _is_
   the lesson's `explanation`. Scores are projections of the text, never
   elicited directly from the model.
2. **Derived, not elicited.** Any numeric judgment (groundedness, quality) is
   computed outside the generating model: v1 = a separate constrained
   "Likert expert" mapping prompt; v2 = embedding cosine similarity against
   reference statements (full SSR — reopens the deferred `sqlite-vec`
   question).
3. **Evidence citation.** The explanation must cite the specific span of
   validator feedback it rests on. An explanation that cites nothing is
   confabulation-by-construction and is rejected before `createLesson`.
4. **Low temperature, small model.** Reflection is a cost on every rejected
   validation; reproducibility matters more than brilliance. Deterministic
   decoding where the provider allows it.

### What stays sacred

- **Validator outcomes stay binary and environmental.** The Beta-Bernoulli is
  fed by ground truth (`approved` from real validation), not model judgment.
- **The classifier anchors.** `failure_class` / `cue` come from the
  deterministic failure classifier; a reflector may not move the retrieval key.
- **The LLM judges explanations against evidence; the environment judges
  interventions against outcomes.** Never the reverse.

### Soft outcomes (later, marked optional)

Tasks without hard validators (style, prose, plan quality) can become
admissible evidence via SSR: judge writes evaluation prose → map to a Likert
distribution → fold into counters as evidence mass `e = Σ p_k·(k−1)/4 ∈ [0,1]`
with fractional updates `helpful += e, harmful += (1−e)`. Beta posteriors
accept fractional counts; Thompson sampling is unchanged; Wilson's _n_ becomes
evidence mass rather than trial count. One-line counter-rule change, but it
shifts the interpretation of _n_ — do it deliberately, not casually.

## 4. `elaborator@1` contract

Interface: the existing policy object (shipped in `df4fbed`) — no observer or
store change:

```js
{
  name: 'elaborator',
  version: 1,
  reflect({ message, failure_class, cue }) -> { explanation, intervention }
}
```

- **Input:** the rejected `VALIDATION_RESULT` message (text + errors). v1 does
  NOT see the wider run trace; see §7.
- **Output:**
  - `explanation` — _why_ the failure happened, abstracted one level above the
    incident, citing the evidence span (§3.3). Cap ~500 chars (matches
    `EXPLANATION_MAX_LENGTH`).
  - `intervention` — _what to do differently_, imperative, transferable beyond
    the specific error text, scoped to `failure_class`. This string is both
    stored and delivered verbatim as guidance, so it must stand alone.
- **Execution:** synchronous inside the observer's `reflectOnRejection`
  try/catch. Timeout or error → `template@1` fallback (already implemented).
  Latency budget: reflector time adds to the rejection→guidance path; keep it
  under a few seconds, small model.
- **Registration:** `REFLECTOR_REGISTRY` entry or per-cluster config
  (`cluster.config.lyo.reflector = 'elaborator@1'`). Swapping is a config
  change, never a code change — that was the point of the interface.

### Failure modes and their answers

| Failure                                 | Answer                                                                                                                               |
| --------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------ |
| Confabulated explanation                | Counters punish it; Wilson quarantines. Induction is honest, so abduction may err.                                                   |
| Prompt injection via validator feedback | Feedback is quoted data inside the reflector prompt, never instructions; output shape is validated (`isValidReflection`) before use. |
| Malformed/refusal output                | `isValidReflection` → `template@1` fallback, warning logged.                                                                         |
| Model unavailable / timeout             | try/catch → `template@1`. A run is never blocked by a learning-layer hiccup (Appendix B.4).                                          |
| Cost creep                              | One small-model call per rejection only; no reflection on accepted validations.                                                      |

## 5. Interchangeability (already shipped)

`src/lyo/reflector-policies.js` defines the contract; the observer resolves
`attachLyoObserver({ reflector })` → `cluster.config.lyo.reflector` →
`template@1` default, and falls back to `template@1` on ANY reflector failure
(unknown id, throw, malformed return). The classifier stays outside the
reflector. `template@1` output is byte-identical to the pre-refactor behavior
(test-pinned). New reflectors (elaborator, future SSR-scored variants,
human-written) require no observer or store changes.

## 6. The A/B protocol: does abstraction lift outcomes?

**Question.** Do elaborator-authored lessons produce higher pass rates on the
cycles they are injected into than template-authored lessons — and do they
transfer across tasks, not just across identical errors?

**Provenance (shipped in `df4fbed`).** Every CREATE/EDIT delta payload records
the authoring reflector id. Join path, all inside the existing store:

```
lesson → lesson_delta(CREATE).payload.reflector   (author)
       → lesson_application                        (per-cycle injection)
       → lesson_application.outcome                (passed/failed)
       → lesson_decision                           (selection snapshot + propensities)
```

**Assignment.** Per cluster: `cluster.config.lyo.reflector = 'template@1' |
'elaborator@1'`. Alternate by run, or split by repository. No code change.

**Estimators.**

- Naive: pass rate of cycles injected with template-lessons vs
  elaborator-lessons vs the null arm (no lesson), with Wilson intervals.
- Off-policy-correct: the ratio-lift / IPW read-side over `lesson_decision`
  propensities — selection probabilities are logged, so the comparison does
  not require randomized injection.

**Decision rule.** Promote `elaborator@1` to the default reflector if the
lower Wilson bound of its lift over template is > 0 after a pre-registered
minimum number of injected cycles per arm; otherwise keep template and treat
the elaborator's cost as unjustified.

**Transfer test.** For each authoring reflector, measure the diversity of
`failure_class` values across _helpful_ applications. Template lessons should
help narrowly (same-error retries); elaborator lessons, if abstraction is
real, should help across a broader class mix.

## 7. Open questions

1. **Trace access.** v1 reflects on the validator message only. Giving the
   reflector a bounded excerpt of the run ledger (recent EDIT/TEST messages)
   would ground explanations in behavior, not just outcomes — at cost and
   injection-risk. Prototype behind a config flag.
2. **Full SSR scoring (v2).** Embedding-mapped Likert for groundedness/quality
   requires an embedding model and likely `sqlite-vec` — the dependency the
   store design deliberately deferred. Revisit when recall or judge quality
   measurably hurts.
3. **Async reflection.** A `LYO_REFLECT_REQUEST` bus topic would let a
   dedicated reflector agent distill asynchronously, landing EDIT deltas later;
   keeps the rejection→guidance path at zero added latency, but the first
   intervention of a cycle then always ships template text. Trade-off to
   measure, not assume.
4. **Multi-failure synthesis.** Several rejections across runs → one merged,
   strengthened lesson is currently the curator's merge pass, which is
   text-preserving by design (no rewrites — the context-collapse rule, §7 of
   the design doc). Whether the curator may _rewrite_ with an LLM is a
   separate decision with its own brevity-bias risks; out of scope here.
5. **Elaboration storage.** If reflectors start producing richer intermediates
   (draft elaborations, citation spans), store them in the delta payload
   rather than widening the lesson table — payloads are the audit layer.

---

## Appendix A. Where each piece lives

| Piece                                         | Location                                                                                |
| --------------------------------------------- | --------------------------------------------------------------------------------------- |
| Reflector interface + `template@1` + registry | `src/lyo/reflector-policies.js`                                                         |
| Observer wiring, fallback, guidance path      | `src/lyo/observer.js` (`reflectOnRejection`, `learnFromRejection`, `attachLyoObserver`) |
| Reflector provenance in deltas                | `src/lyo/lesson-store.js` (`createLesson`, CREATE/EDIT payloads)                        |
| Tests (10)                                    | `tests/unit/lyo-reflector-policy.test.js`                                               |
| Personal-layer delivery of lessons            | `scripts/lyo-kimi-session-hook.js` (kimi-code SessionStart hook)                        |
| Selection policies (sibling pattern)          | `src/lyo/selection-policies.js`                                                         |
