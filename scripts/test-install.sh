#!/bin/bash
# Test zeroshot installation on any platform
# Run: ./scripts/test-install.sh

set -e

echo "=== Zeroshot Install Test ==="
echo "Platform: $(uname -s) $(uname -m)"
echo "Node: $(node --version)"
echo "npm: $(npm --version)"
echo ""

echo "1. Installing dependencies..."
npm install

echo ""
echo "2. Linking CLI..."
npm link

echo ""
echo "3. Testing CLI commands..."

echo "   zeroshot --version"
zeroshot --version

echo ""
echo "   zeroshot --help"
zeroshot --help | head -20

echo ""
echo "   zeroshot list"
zeroshot list 2>/dev/null || echo "   (no clusters yet - this is expected)"

echo ""
echo "   zeroshot config list"
zeroshot config list

echo ""
echo "=== ALL TESTS PASSED ==="
echo "Zeroshot installed successfully on $(uname -s)!"
