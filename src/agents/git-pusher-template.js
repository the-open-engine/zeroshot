/**
 * Git Pusher Agent Template
 *
 * Generates platform-specific git-pusher agent configurations.
 * Eliminates duplication across github/gitlab/azure JSON files.
 *
 * Single source of truth for:
 * - Trigger logic (validation consensus detection)
 * - Agent structure (id, role, modelLevel, output)
 * - Prompt template with platform-specific commands
 */

/**
 * Shared trigger logic for detecting when all validators have approved.
 * This is the SINGLE source of truth - no more duplicating across 3 JSON files.
 */
const SHARED_TRIGGER_SCRIPT = `const validators = cluster.getAgentsByRole('validator');
const lastPush = ledger.findLast({ topic: 'IMPLEMENTATION_READY' });
if (!lastPush) return false;
if (validators.length === 0) return true;
const results = ledger.query({ topic: 'VALIDATION_RESULT', since: lastPush.timestamp });
if (results.length < validators.length) return false;
const allApproved = results.every(r => r.content?.data?.approved === 'true' || r.content?.data?.approved === true);
if (!allApproved) return false;
const hasRealEvidence = results.every(r => {
  const criteria = r.content?.data?.criteriaResults || [];
  return criteria.every(c => {
    return c.evidence?.command && typeof c.evidence?.exitCode === 'number' && c.evidence?.output?.length > 10;
  });
});
return hasRealEvidence;`;

/**
 * Platform-specific CLI commands and terminology
 */
const PLATFORM_CONFIGS = {
  github: {
    prName: 'PR',
    prNameLower: 'pull request',
    createCmd: 'gh pr create --title "feat: {{issue_title}}" --body "Closes #{{issue_number}}"',
    mergeCmd: 'gh pr merge --merge --auto',
    mergeFallbackCmd: 'gh pr merge --merge',
    prUrlExample: 'https://github.com/owner/repo/pull/123',
    outputFields: { urlField: 'pr_url', numberField: 'pr_number', mergedField: 'merged' },
  },
  gitlab: {
    prName: 'MR',
    prNameLower: 'merge request',
    createCmd:
      'glab mr create --title "feat: {{issue_title}}" --description "Closes #{{issue_number}}"',
    mergeCmd: 'glab mr merge --auto-merge',
    mergeFallbackCmd: 'glab mr merge',
    prUrlExample: 'https://gitlab.com/owner/repo/-/merge_requests/123',
    outputFields: { urlField: 'mr_url', numberField: 'mr_number', mergedField: 'merged' },
  },
  'azure-devops': {
    prName: 'PR',
    prNameLower: 'pull request',
    createCmd:
      'az repos pr create --title "feat: {{issue_title}}" --description "Closes #{{issue_number}}"',
    mergeCmd: 'az repos pr update --id <PR_ID> --auto-complete true',
    mergeFallbackCmd: 'az repos pr update --id <PR_ID> --status completed',
    prUrlExample: 'https://dev.azure.com/org/project/_git/repo/pullrequest/123',
    outputFields: {
      urlField: 'pr_url',
      numberField: 'pr_number',
      mergedField: 'merged',
      autoCompleteField: 'auto_complete',
    },
    // Azure requires extracting PR ID from create output
    requiresPrIdExtraction: true,
  },
};

/**
 * Generate the prompt for a specific platform
 * @param {Object} config - Platform configuration from PLATFORM_CONFIGS
 * @returns {string} The complete prompt with platform-specific commands
 */
function generatePrompt(config) {
  const {
    prName,
    prNameLower,
    createCmd,
    mergeCmd,
    mergeFallbackCmd,
    prUrlExample,
    outputFields,
    requiresPrIdExtraction,
  } = config;

  // Azure-specific instructions for PR ID extraction
  const azurePrIdNote = requiresPrIdExtraction
    ? `\n\n💡 IMPORTANT: The output will contain the PR ID. You MUST extract it for the next step.
Look for output like: "Created PR 123" or parse the URL for the PR number.
Save the PR ID to a variable for step 6.`
    : '';

  // Azure uses different merge terminology
  const mergeDescription = requiresPrIdExtraction
    ? 'SET AUTO-COMPLETE (MANDATORY - THIS IS NOT OPTIONAL)'
    : `MERGE THE ${prName} (MANDATORY - THIS IS NOT OPTIONAL)`;

  const mergeExplanation = requiresPrIdExtraction
    ? `Replace <PR_ID> with the actual PR number from step 5.
This enables auto-complete (auto-merge when CI passes).

If auto-complete is not available or you need to merge immediately:`
    : `This sets auto-merge. If it fails (e.g., no auto-merge enabled), try:`;

  const postMergeStatus = requiresPrIdExtraction
    ? 'PR IS CREATED AND AUTO-COMPLETE IS SET'
    : `${prName} IS MERGED`;

  const finalOutputNote = requiresPrIdExtraction
    ? `ONLY after the PR is created and auto-complete is set, output:
\`\`\`json
{"${outputFields.urlField}": "${prUrlExample}", "${outputFields.numberField}": 123, "merged": false, "auto_complete": true}
\`\`\`

If truly no changes exist, output:
\`\`\`json
{"${outputFields.urlField}": null, "${outputFields.numberField}": null, "merged": false, "auto_complete": false}
\`\`\``
    : `ONLY after the ${prName} is MERGED, output:
\`\`\`json
{"${outputFields.urlField}": "${prUrlExample}", "${outputFields.numberField}": 123, "merged": true}
\`\`\`

If truly no changes exist, output:
\`\`\`json
{"${outputFields.urlField}": null, "${outputFields.numberField}": null, "merged": false}
\`\`\``;

  return `🚨 CRITICAL: ALL VALIDATORS APPROVED. YOU MUST CREATE A ${prName} AND GET IT MERGED. DO NOT STOP UNTIL THE ${prName} IS MERGED. 🚨

## MANDATORY STEPS - EXECUTE EACH ONE IN ORDER - DO NOT SKIP ANY STEP

### STEP 1: Stage ALL changes (MANDATORY)
\`\`\`bash
git add -A
\`\`\`
Run this command. Do not skip it.

### STEP 2: Check what's staged
\`\`\`bash
git status
\`\`\`
Run this. If nothing to commit, output JSON with ${outputFields.urlField}: null and stop.

### STEP 3: Commit the changes (MANDATORY if there are changes)
\`\`\`bash
git commit -m "feat: implement #{{issue_number}} - {{issue_title}}"
\`\`\`
Run this command. Do not skip it.

### STEP 4: Push to origin (MANDATORY)
\`\`\`bash
git push -u origin HEAD
\`\`\`
Run this. If it fails, check the error and fix it.

⚠️ AFTER PUSH YOU ARE NOT DONE! CONTINUE TO STEP 5! ⚠️

### STEP 5: CREATE THE ${prName.toUpperCase()} (MANDATORY - YOU MUST RUN THIS COMMAND)
\`\`\`bash
${createCmd}
\`\`\`
🚨 YOU MUST RUN \`${createCmd.split(' ').slice(0, 3).join(' ')}\`! Outputting a link is NOT creating a ${prName}! 🚨
The push output shows a "Create a ${prNameLower}" link - IGNORE IT.
You MUST run the \`${createCmd.split(' ').slice(0, 3).join(' ')}\` command above.${requiresPrIdExtraction ? '' : ` Save the actual ${prName} URL from the output.`}${azurePrIdNote}

⚠️ AFTER ${prName} CREATION YOU ARE NOT DONE! CONTINUE TO STEP 6! ⚠️

### STEP 6: ${mergeDescription}
\`\`\`bash
${mergeCmd}
\`\`\`
${mergeExplanation}
\`\`\`bash
${mergeFallbackCmd}
\`\`\`

🚨 IF MERGE FAILS DUE TO CONFLICTS - YOU MUST RESOLVE THEM:
a) Pull latest main and rebase:
   \`\`\`bash
   git fetch origin main
   git rebase origin/main
   \`\`\`
b) If conflicts appear - RESOLVE THEM IMMEDIATELY:
   - Read the conflicting files
   - Make intelligent decisions about what code to keep
   - Edit the files to resolve conflicts
   - \`git add <resolved-files>\`
   - \`git rebase --continue\`
c) Force push the resolved branch:
   \`\`\`bash
   git push --force-with-lease
   \`\`\`
d) Retry merge:
   \`\`\`bash
   ${mergeFallbackCmd}
   \`\`\`

REPEAT UNTIL MERGED. DO NOT GIVE UP. DO NOT SKIP. THE ${prName} MUST BE ${requiresPrIdExtraction ? 'SET TO AUTO-COMPLETE' : 'MERGED'}.
If merge is blocked by CI, wait and retry. ${requiresPrIdExtraction ? 'The auto-complete will merge when CI passes.' : 'If blocked by reviews, set auto-merge.'}

## CRITICAL RULES
- Execute EVERY step in order (1, 2, 3, 4, 5, 6)
- Do NOT skip git add -A
- Do NOT skip git commit
- Do NOT skip ${createCmd.split(' ').slice(0, 3).join(' ')} - THE TASK IS NOT DONE UNTIL ${prName} EXISTS
- Do NOT skip ${mergeCmd.split(' ').slice(0, 4).join(' ')} - THE TASK IS NOT DONE UNTIL ${postMergeStatus}${requiresPrIdExtraction ? '\n- MUST extract PR ID from step 5 output to use in step 6' : ''}
- If push fails, debug and fix it
- If ${prName} creation fails, debug and fix it
- If ${requiresPrIdExtraction ? 'auto-complete' : 'merge'} fails, debug and fix it
- DO NOT OUTPUT JSON UNTIL ${postMergeStatus}
- A link from git push is NOT a ${prName} - you must run ${createCmd.split(' ').slice(0, 3).join(' ')}

## Final Output
${finalOutputNote}`;
}

/**
 * Generate a git-pusher agent configuration for a specific platform
 *
 * @param {string} platform - Platform ID ('github', 'gitlab', 'azure-devops')
 * @returns {Object} Agent configuration object
 * @throws {Error} If platform is not supported
 */
function generateGitPusherAgent(platform) {
  const config = PLATFORM_CONFIGS[platform];

  if (!config) {
    const supported = Object.keys(PLATFORM_CONFIGS).join(', ');
    throw new Error(`Unsupported platform '${platform}'. Supported: ${supported}`);
  }

  return {
    id: 'git-pusher',
    role: 'completion-detector',
    modelLevel: 'level2',
    triggers: [
      {
        topic: 'VALIDATION_RESULT',
        logic: {
          engine: 'javascript',
          script: SHARED_TRIGGER_SCRIPT,
        },
        action: 'execute_task',
      },
    ],
    prompt: generatePrompt(config),
    hooks: {
      onComplete: {
        action: 'publish_message',
        config: {
          topic: 'CLUSTER_COMPLETE',
          content: {
            data: { reason: 'git-pusher-complete' },
          },
        },
      },
    },
    output: {
      topic: 'PR_CREATED',
      publishAfter: 'CLUSTER_COMPLETE',
    },
  };
}

/**
 * Get list of supported platforms for git-pusher
 * @returns {string[]} Array of platform IDs
 */
function getSupportedPlatforms() {
  return Object.keys(PLATFORM_CONFIGS);
}

/**
 * Check if a platform supports git-pusher (PR/MR creation)
 * @param {string} platform - Platform ID
 * @returns {boolean}
 */
function isPlatformSupported(platform) {
  return platform in PLATFORM_CONFIGS;
}

module.exports = {
  generateGitPusherAgent,
  getSupportedPlatforms,
  isPlatformSupported,
  // Export for testing
  SHARED_TRIGGER_SCRIPT,
  PLATFORM_CONFIGS,
};
