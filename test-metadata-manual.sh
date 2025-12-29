#!/bin/bash
# Manual test: spawn conductor cluster and check debug logs for metadata propagation

set -e

# Clean up any existing test clusters
rm -rf ~/.zeroshot/cluster-test-metadata-*

# Run in background, kill after 15s
timeout 15s node src/cli.js run "invalid-command" 2>&1 | tee /tmp/zeroshot-metadata-test.log || true

echo ""
echo "========= ANALYSIS ========="
echo ""
echo "1. How many times did junior-conductor execute?"
grep -c "junior-conductor.*AGENT_LIFECYCLE.*task_started" /tmp/zeroshot-metadata-test.log || echo "0"

echo ""
echo "2. Republished ISSUE_OPENED metadata:"
grep "DEBUG _opPublish.*ISSUE_OPENED" /tmp/zeroshot-metadata-test.log || echo "NOT FOUND"

echo ""
echo "3. Published message metadata:"
grep "messageToPublish.metadata" /tmp/zeroshot-metadata-test.log || echo "NOT FOUND"
