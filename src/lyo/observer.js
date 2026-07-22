const fs = require('fs');
const os = require('os');
const path = require('path');
const { USER_GUIDANCE_AGENT } = require('../guidance-topics');
const LessonStore = require('./lesson-store');
const { classifyValidationFailure } = require('./failure-classifier');
const {
  TEMPLATE_REFLECTOR,
  reflectorId,
  isValidReflection,
  resolveReflector,
} = require('./reflector-policies');

const LYO_INTERVENTION_TOPIC = 'LYO_INTERVENTION';
const LYO_FEEDBACK_TOPIC = 'LYO_FEEDBACK';
const LYO_SENDER = 'lyo';

function isLyoEnabled(cluster) {
  return cluster?.config?.lyo?.enabled === true;
}

function findImplementationAgentId(cluster) {
  return cluster?.agents?.find((agent) => agent.config?.role === 'implementation')?.id || null;
}

function isRejectedValidation(message) {
  if (message?.topic !== 'VALIDATION_RESULT') return false;
  return message.content?.data?.approved === false;
}

function getValidationOutcome(message) {
  const approved = message.content?.data?.approved;
  if (approved === true) return 'accepted';
  if (approved === false) return 'rejected';
  return 'unknown';
}

function publishInterventionFeedback({ messageBus, cluster, pendingIntervention, message }) {
  messageBus.publish({
    cluster_id: cluster.id,
    topic: LYO_FEEDBACK_TOPIC,
    sender: LYO_SENDER,
    content: {
      text: `Intervention feedback: ${getValidationOutcome(message)}.`,
      data: {
        intervention_id: pendingIntervention.intervention_id,
        trigger_message_id: pendingIntervention.trigger_message_id,
        feedback_message_id: message.id,
        target_agent_id: pendingIntervention.target_agent_id,
      },
    },
    metadata: {
      source: 'lyo_observer',
    },
  });
}

/**
 * Store resolution order: cluster.config.lyo.storePath ->
 * ZEROSHOT_LYO_STORE_PATH -> <cwd>/.zeroshot/lyo-lessons.db (when the cluster
 * has a cwd) -> <storageDir>/lyo-lessons.db (storageDir defaults to
 * ~/.zeroshot). The lesson store is LYO's OWN file: it is never the per-run
 * ledger DB, so lessons survive across runs (cross-run recall).
 */
function resolveLessonStorePath({ cluster, storageDir }) {
  const configuredPath = cluster?.config?.lyo?.storePath;
  if (configuredPath) {
    return configuredPath;
  }
  if (process.env.ZEROSHOT_LYO_STORE_PATH) {
    return process.env.ZEROSHOT_LYO_STORE_PATH;
  }
  const cwd = cluster?.config?.cwd || cluster?.cwd;
  if (cwd) {
    return path.join(cwd, '.zeroshot', 'lyo-lessons.db');
  }
  return path.join(storageDir || path.join(os.homedir(), '.zeroshot'), 'lyo-lessons.db');
}

// Blast-radius containment (design doc Appendix B.4): a broken lesson store
// must never block a run. On any failure the observer continues with
// store = null (degraded mode: interventions still publish, learning skipped).
function openLessonStore({ cluster, lessonStore, storageDir }) {
  if (lessonStore) {
    return { store: lessonStore, owned: false };
  }
  try {
    const storePath = resolveLessonStorePath({ cluster, storageDir });
    fs.mkdirSync(path.dirname(storePath), { recursive: true });
    return { store: new LessonStore(storePath), owned: true };
  } catch (error) {
    console.warn('[lyo] lesson store unavailable, running degraded:', error.message);
    return { store: null, owned: false };
  }
}

// Run the configured reflector policy over a rejection. ANY failure — unknown
// registry id, reflector throw, malformed return — degrades to template@1
// (pure, never throws), so the guidance path always has an intervention text.
function reflectOnRejection({ message, failure_class, cue, reflectorRef }) {
  const fallback = () => ({
    reflection: {
      ...TEMPLATE_REFLECTOR.reflect({ message }),
      reflector_id: reflectorId(TEMPLATE_REFLECTOR),
    },
    reflector: TEMPLATE_REFLECTOR,
  });
  try {
    const reflector = resolveReflector(reflectorRef);
    if (typeof reflector.reflectAsync === 'function') {
      // Async reflector (e.g. elaborator@1): guidance ships template text
      // synchronously (zero added latency); the caller fires reflectAsync to
      // enrich the store for future cycles (docs/lyo-reflector-design.md §4).
      return { ...fallback(), reflector, asyncPending: true };
    }
    const reflection = reflector.reflect({ message, failure_class, cue });
    if (!isValidReflection(reflection)) {
      console.warn(
        `[lyo] reflector ${reflectorId(reflector)} returned a malformed reflection, using template fallback`
      );
      return fallback();
    }
    return {
      reflection: { ...reflection, reflector_id: reflectorId(reflector) },
      reflector,
    };
  } catch (error) {
    console.warn('[lyo] reflector failed, using template fallback:', error.message);
    return fallback();
  }
}

// Fire-and-forget enrichment for async reflectors: the model's distilled
// reflection is persisted for FUTURE cycles; on any failure a template@1
// lesson is stored instead, so evidence continuity survives a down model.
// Returns a promise that NEVER rejects (containment: Appendix B.4) so callers
// may safely leave it floating; the onEnrichment attach option exposes it for
// tests.
function enrichStoreAsync({ store, cluster, reflector, message, failure_class, cue }) {
  return Promise.resolve()
    .then(() => reflector.reflectAsync({ message, failure_class, cue }))
    .then((reflection) => {
      if (!isValidReflection(reflection)) {
        throw new Error(`malformed reflection from ${reflectorId(reflector)}`);
      }
      persistReflection({
        store,
        cluster,
        failure_class,
        cue,
        reflection: { ...reflection, reflector_id: reflectorId(reflector) },
      });
    })
    .catch((error) => {
      console.warn('[lyo] async reflector failed, storing template reflection:', error.message);
      try {
        persistReflection({
          store,
          cluster,
          failure_class,
          cue,
          reflection: {
            ...TEMPLATE_REFLECTOR.reflect({ message }),
            reflector_id: reflectorId(TEMPLATE_REFLECTOR),
          },
        });
      } catch (persistError) {
        console.warn('[lyo] failed to store fallback reflection:', persistError.message);
      }
    });
}

// Persist one reflection as a create-or-merge lesson (design doc §2).
function persistReflection({ store, cluster, failure_class, cue, reflection }) {
  store.createLesson({
    failure_class,
    trigger_cue: cue,
    explanation: reflection.explanation,
    intervention: reflection.intervention,
    run_id: cluster.id,
    actor: 'reflector',
    reflector: reflection.reflector_id,
  });
}

// Reflector role (design doc §2): the REFLECTION (abduction — explanation +
// intervention) is produced upstream by a pluggable reflector policy and
// passed in; this function optionally persists it (persistCreate — skipped
// when an async reflector will persist its own reflection later), then
// Thompson-selects lessons for this failure class, logs the decision (full
// candidate snapshot + selection propensities, §5.3), and records one
// application row per selected lesson (grounded attribution).
function learnFromRejection({
  store,
  cluster,
  message,
  messageBus,
  selectionPolicy = null,
  failure_class,
  cue,
  reflection,
  persistCreate = true,
}) {
  if (persistCreate) {
    persistReflection({ store, cluster, failure_class, cue, reflection });
  }
  // The just-created candidate competes via its Beta(1,1) draw — intended
  // exploration, not a shortcut (design doc §4.2).
  const { selected, candidates, null_arm, policy } = store.selectWithDecision({
    failure_class,
    limit: 2,
    policy: selectionPolicy,
  });
  // Cycle index = interventions already published in this run + 1 (this
  // decision's intervention is published right after learnFromRejection
  // returns). Defensive: a bus without count() still gets a decision row,
  // with cycle_index NULL.
  let cycleIndex = null;
  try {
    cycleIndex = messageBus.count({ cluster_id: cluster.id, topic: LYO_INTERVENTION_TOPIC }) + 1;
  } catch {
    cycleIndex = null;
  }
  const decision = store.recordDecision({
    run_id: cluster.id,
    trigger_message_id: message.id,
    cycle_index: cycleIndex,
    failure_class,
    task_cue: cue,
    candidates,
    selected: selected.map((lesson) => ({
      lesson_id: lesson.lesson_id,
      score: lesson.sampled_score,
    })),
    null_arm,
    policy,
  });
  return selected.map((lesson) => {
    const application = store.recordApplication({
      lesson_id: lesson.lesson_id,
      run_id: cluster.id,
      trigger_message_id: message.id,
      task_cue: cue,
      sampled_score: lesson.sampled_score,
      decision_id: decision.decision_id,
    });
    const candidate = candidates.find((entry) => entry.lesson_id === lesson.lesson_id);
    return {
      lesson,
      application,
      decision_id: decision.decision_id,
      propensity: candidate ? candidate.propensity : null,
    };
  });
}

// Appended AFTER the existing guidance text; the prefix stays verbatim.
function buildLessonSection(lessonApplications) {
  if (!lessonApplications || lessonApplications.length === 0) {
    return '';
  }
  const lines = lessonApplications.map(
    ({ lesson }) => `- [${lesson.lesson_id}] ${lesson.intervention}`
  );
  return `\n\nLessons from past failures (LYO):\n${lines.join('\n')}`;
}

function summarizeLessons(lessonApplications) {
  if (!lessonApplications) {
    return null;
  }
  return lessonApplications.map(({ lesson, application, decision_id, propensity }) => ({
    lesson_id: lesson.lesson_id,
    application_id: application.application_id,
    failure_class: lesson.failure_class,
    sampled_score: lesson.sampled_score,
    decision_id: decision_id ?? null,
    propensity: propensity ?? null,
  }));
}

function attachLyoObserver({
  messageBus,
  cluster,
  lessonStore,
  storageDir,
  selectionPolicy,
  reflector,
  onEnrichment,
}) {
  if (!messageBus) {
    throw new Error('attachLyoObserver: messageBus is required');
  }

  if (!cluster?.id) {
    throw new Error('attachLyoObserver: cluster.id is required');
  }

  if (!isLyoEnabled(cluster)) {
    return () => {};
  }

  const { store, owned } = openLessonStore({ cluster, lessonStore, storageDir });
  // Selection policy resolution order (§4.2): explicit attach option ->
  // cluster.config.lyo.policy (registry id string, e.g. 'thompson-beta@1')
  // -> default. An unknown id throws inside the store per decision and
  // degrades to the existing no-lessons warning path.
  const resolvedSelectionPolicy = selectionPolicy ?? cluster.config.lyo.policy ?? null;
  // Reflector policy resolution order (§2): explicit attach option ->
  // cluster.config.lyo.reflector (registry id string, e.g. 'template@1')
  // -> default. Unknown ids and reflector failures fall back to template@1
  // inside reflectOnRejection; learning never blocks a run.
  const resolvedReflector = reflector ?? cluster.config.lyo.reflector ?? null;
  let pendingIntervention = null;

  const unsubscribe = messageBus.subscribe((message) => {
    if (message.cluster_id !== cluster.id || message.topic !== 'VALIDATION_RESULT') {
      return;
    }

    if (pendingIntervention && message.id !== pendingIntervention.trigger_message_id) {
      publishInterventionFeedback({ messageBus, cluster, pendingIntervention, message });
      pendingIntervention = null;

      // Validator rule (design doc §5.1): the resolving validation counts the
      // cycle's applications. Learning must never block a run, so failures
      // here degrade to a warning.
      if (store) {
        const approved = message.content?.data?.approved;
        if (approved === true || approved === false) {
          try {
            store.applyValidationOutcome({
              run_id: cluster.id,
              outcome: approved ? 'passed' : 'failed',
            });
          } catch (error) {
            console.warn('[lyo] failed to apply validation outcome:', error.message);
          }
        }
      }
    }

    if (!isRejectedValidation(message)) {
      return;
    }

    const targetAgentId = findImplementationAgentId(cluster);
    if (!targetAgentId) {
      return;
    }

    // Grounding first (deterministic classifier), then abduction (pluggable
    // reflector). Both run even in degraded mode (no store): the reflector's
    // intervention is also the base guidance text delivered to the agent.
    const { failure_class, cue } = classifyValidationFailure(message);
    const {
      reflection,
      reflector: resolvedReflectorPolicy,
      asyncPending,
    } = reflectOnRejection({
      message,
      failure_class,
      cue,
      reflectorRef: resolvedReflector,
    });

    let lessonApplications = null;
    if (store) {
      try {
        lessonApplications = learnFromRejection({
          store,
          cluster,
          message,
          messageBus,
          selectionPolicy: resolvedSelectionPolicy,
          failure_class,
          cue,
          reflection,
          persistCreate: !asyncPending,
        });
      } catch (error) {
        console.warn('[lyo] lesson pipeline failed, continuing without lessons:', error.message);
        lessonApplications = null;
      }

      if (asyncPending) {
        // Async reflector: enrich the store for future cycles, off the hot
        // path. The promise never rejects; onEnrichment exposes it to tests.
        const enrichment = enrichStoreAsync({
          store,
          cluster,
          reflector: resolvedReflectorPolicy,
          message,
          failure_class,
          cue,
        });
        if (typeof onEnrichment === 'function') {
          onEnrichment(enrichment);
        }
      }
    }
    const lessons = summarizeLessons(lessonApplications);

    const intervention = messageBus.publish({
      cluster_id: cluster.id,
      topic: LYO_INTERVENTION_TOPIC,
      sender: LYO_SENDER,
      content: {
        text: `Validation rejected; queued implementation guidance for ${targetAgentId}.`,
        data: {
          trigger_message_id: message.id,
          target_agent_id: targetAgentId,
          lessons,
        },
      },
      metadata: {
        source: 'lyo_observer',
      },
    });

    pendingIntervention = {
      intervention_id: intervention?.id || null,
      trigger_message_id: message.id,
      target_agent_id: targetAgentId,
    };

    messageBus.publish({
      cluster_id: cluster.id,
      topic: USER_GUIDANCE_AGENT,
      sender: LYO_SENDER,
      target_agent_id: targetAgentId,
      content: {
        text: `${reflection.intervention}${buildLessonSection(lessonApplications)}`,
        data: {
          trigger_message_id: message.id,
          intervention_id: intervention?.id || null,
          lessons,
        },
      },
      metadata: {
        source: 'lyo_observer',
      },
    });
  });

  let detached = false;
  return () => {
    if (detached) {
      return;
    }
    detached = true;
    unsubscribe();
    // Only close a store the observer created itself; injected stores are
    // owned by the caller and stay open.
    if (owned && store) {
      try {
        store.close();
      } catch (error) {
        console.warn('[lyo] failed to close lesson store:', error.message);
      }
    }
  };
}

module.exports = {
  attachLyoObserver,
};
