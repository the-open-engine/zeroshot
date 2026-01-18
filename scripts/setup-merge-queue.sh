#!/bin/bash
# Setup GitHub merge queue and branch protection for zeroshot
# Run once after creating the repo or to update settings

set -e

REPO="covibes/zeroshot"

echo "╔═══════════════════════════════════════════════════════════════════╗"
echo "║  Setting up merge queue for $REPO                         ║"
echo "╚═══════════════════════════════════════════════════════════════════╝"
echo ""

# Check gh is authenticated
if ! gh auth status &>/dev/null; then
  echo "❌ ERROR: Not authenticated with GitHub CLI"
  echo "   Run: gh auth login"
  exit 1
fi

# Check we have admin access
if ! gh api "repos/$REPO" --jq '.permissions.admin' | grep -q true; then
  echo "❌ ERROR: You need admin access to $REPO"
  exit 1
fi

echo "✓ Authenticated with admin access"
echo ""

# ============================================================================
# Configure 'dev' branch protection (merge target)
# ============================================================================

echo "→ Configuring 'dev' branch protection..."

gh api --method PUT "repos/$REPO/branches/dev/protection" \
  --input - <<EOF
{
  "required_status_checks": {
    "strict": true,
    "contexts": ["check", "install-matrix (ubuntu-latest, 20)", "install-matrix (macos-latest, 20)"]
  },
  "enforce_admins": false,
  "required_pull_request_reviews": null,
  "restrictions": null,
  "allow_force_pushes": false,
  "allow_deletions": false,
  "required_linear_history": true,
  "required_conversation_resolution": false
}
EOF

echo "✓ 'dev' branch protection configured"

# Enable merge queue for dev branch
echo "→ Enabling merge queue for 'dev' branch..."

# Note: Merge queue requires GitHub Enterprise or public repos with Actions
# Using the ruleset API which supports merge queue
gh api --method POST "repos/$REPO/rulesets" \
  --input - <<EOF 2>/dev/null || echo "   (ruleset may already exist)"
{
  "name": "dev-merge-queue",
  "target": "branch",
  "enforcement": "active",
  "conditions": {
    "ref_name": {
      "include": ["refs/heads/dev"],
      "exclude": []
    }
  },
  "rules": [
    {
      "type": "merge_queue",
      "parameters": {
        "check_response_timeout_minutes": 30,
        "grouping_strategy": "ALLGREEN",
        "max_entries_to_build": 5,
        "max_entries_to_merge": 5,
        "merge_method": "SQUASH",
        "min_entries_to_merge": 1,
        "min_entries_to_merge_wait_minutes": 1
      }
    }
  ]
}
EOF

echo "✓ Merge queue enabled for 'dev'"

# ============================================================================
# Configure 'main' branch protection (release branch)
# ============================================================================

echo "→ Configuring 'main' branch protection..."

gh api --method PUT "repos/$REPO/branches/main/protection" \
  --input - <<EOF
{
  "required_status_checks": {
    "strict": true,
    "contexts": ["check", "install-matrix (ubuntu-latest, 20)", "install-matrix (macos-latest, 20)"]
  },
  "enforce_admins": false,
  "required_pull_request_reviews": {
    "required_approving_review_count": 1,
    "dismiss_stale_reviews": true,
    "require_code_owner_reviews": false
  },
  "restrictions": null,
  "allow_force_pushes": false,
  "allow_deletions": false,
  "required_linear_history": true,
  "required_conversation_resolution": false
}
EOF

echo "✓ 'main' branch protection configured"

# ============================================================================
# Configure repository settings
# ============================================================================

echo "→ Configuring repository settings..."

gh api --method PATCH "repos/$REPO" \
  --input - <<EOF
{
  "allow_squash_merge": true,
  "allow_merge_commit": false,
  "allow_rebase_merge": false,
  "squash_merge_commit_title": "PR_TITLE",
  "squash_merge_commit_message": "PR_BODY",
  "delete_branch_on_merge": true,
  "allow_auto_merge": true
}
EOF

echo "✓ Repository settings configured"

# ============================================================================
# Summary
# ============================================================================

echo ""
echo "╔═══════════════════════════════════════════════════════════════════╗"
echo "║  ✓ Merge queue setup complete!                                    ║"
echo "╚═══════════════════════════════════════════════════════════════════╝"
echo ""
echo "Workflow:"
echo "  feature-branch (local)"
echo "  ↓"
echo "  pre-push hook → lint + typecheck (~5s)"
echo "  ↓"
echo "  push to origin/feature-branch"
echo "  ↓"
echo "  gh pr create --base dev"
echo "  ↓"
echo "  CI runs tests on PR branch"
echo "  ↓"
echo "  gh pr merge --auto --squash → enters merge queue"
echo "  ↓"
echo "  Queue rebases PR on latest dev + runs CI again"
echo "  ↓"
echo "  Merge to dev (only if CI passes on rebased code)"
echo ""
echo "Release workflow:"
echo "  gh pr create --base main --head dev --title \"Release\""
echo "  → CI passes → merge → semantic-release publishes"
echo ""
