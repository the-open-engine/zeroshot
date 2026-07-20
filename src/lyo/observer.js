const fs = require('fs');
const os = require('os');
const path = require('path');
const { USER_GUIDANCE_AGENT } = require('../guidance-topics');
const LessonStore = require('./lesson-store');
const { classifyValidationFailure } = require('./failure-classifier');

const LYO_INTERVENTION_TOPIC = 'LYO_INTERVENTION';
const LYO_FEEDBACK_TOPIC = 'LYO_FEEDBACK';
const LYO_SENDER = 'lyo';
const EXPLANATION_MAX_LENGTH = 500;

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

function formatValidationFeedback(message) {
  const parts = [];
  const text = message.content?.text;
  const errors = message.content?.data?.errors;

  if (text) {
    parts.push(text);
  }

  if (Array.isArray(errors) && errors.length > 0) {
    parts.push(`Errors:\n${errors.map((error) => `- ${error}`).join('\n')}`);
  }

  return parts.join('\n\n') || 'Validator rejected the last result without details.';
}

function buildGuidanceText(message) {
  return `Address the validator feedback before retrying.\n\nLatest validation:\n${formatValidationFeedback(message)}`;
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

function truncate(text, maxLength) {
  const value = String(text || '');
  return value.length > maxLength ? `${value.slice(0, maxLength - 3)}...` : value;
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

// Reflector role (design doc §2): classify the failure, create-or-merge the
// lesson, Thompson-select lessons for this failure class, and record one
// application row per selected lesson (grounded attribution).
function learnFromRejection({ store, cluster, message }) {
  const { failure_class, cue } = classifyValidationFailure(message);
  store.createLesson({
    failure_class,
    trigger_cue: cue,
    explanation: truncate(formatValidationFeedback(message), EXPLANATION_MAX_LENGTH),
    intervention: buildGuidanceText(message),
    run_id: cluster.id,
    actor: 'reflector',
  });
  // The just-created candidate competes via its Beta(1,1) draw — intended
  // exploration, not a shortcut (design doc §4.2).
  const selected = store.selectLessons({ failure_class, limit: 2 });
  return selected.map((lesson) => {
    const application = store.recordApplication({
      lesson_id: lesson.lesson_id,
      run_id: cluster.id,
      trigger_message_id: message.id,
      task_cue: cue,
      sampled_score: lesson.sampled_score,
    });
    return { lesson, application };
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
  return lessonApplications.map(({ lesson, application }) => ({
    lesson_id: lesson.lesson_id,
    application_id: application.application_id,
    failure_class: lesson.failure_class,
    sampled_score: lesson.sampled_score,
  }));
}

function attachLyoObserver({ messageBus, cluster, lessonStore, storageDir }) {
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

    let lessonApplications = null;
    if (store) {
      try {
        lessonApplications = learnFromRejection({ store, cluster, message });
      } catch (error) {
        console.warn('[lyo] lesson pipeline failed, continuing without lessons:', error.message);
        lessonApplications = null;
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
        text: `${buildGuidanceText(message)}${buildLessonSection(lessonApplications)}`,
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
