#!/bin/bash
# Build zeroshot-cluster-base Docker image with retry logic
#
# Usage: ./build-image.sh [--tag IMAGE_NAME] [--max-retries N]
#
# Implements exponential backoff retry for network-related build failures

set -e

# Default configuration
IMAGE_NAME="zeroshot-cluster-base"
MAX_RETRIES=3
BASE_DELAY=2  # seconds
DOCKERFILE_PATH="docker/zeroshot-cluster/Dockerfile"

# Parse arguments
while [[ $# -gt 0 ]]; do
  case $1 in
    --tag)
      IMAGE_NAME="$2"
      shift 2
      ;;
    --max-retries)
      MAX_RETRIES="$2"
      shift 2
      ;;
    *)
      echo "Unknown option: $1"
      echo "Usage: $0 [--tag IMAGE_NAME] [--max-retries N]"
      exit 1
      ;;
  esac
done

# Ensure we're in the zeroshot/cluster directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

# Verify Dockerfile exists
if [[ ! -f "$DOCKERFILE_PATH" ]]; then
  echo "❌ Error: Dockerfile not found at $DOCKERFILE_PATH"
  exit 1
fi

echo "Building Docker image: $IMAGE_NAME"
echo "Max retries: $MAX_RETRIES"
echo "Dockerfile: $DOCKERFILE_PATH"
echo ""

# Retry loop with exponential backoff
for attempt in $(seq 1 $MAX_RETRIES); do
  echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
  echo "🔨 Build attempt $attempt/$MAX_RETRIES"
  echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

  if docker build -t "$IMAGE_NAME" -f "$DOCKERFILE_PATH" .; then
    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "✅ Build successful!"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo ""
    echo "Image: $IMAGE_NAME"
    echo ""
    echo "Usage:"
    echo "  zeroshot run <task> --docker"
    echo "  zeroshot run <issue-number> --docker"
    echo ""
    exit 0
  fi

  # Build failed
  echo ""
  echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
  echo "❌ Build failed (attempt $attempt/$MAX_RETRIES)"
  echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

  if [[ $attempt -lt $MAX_RETRIES ]]; then
    # Calculate exponential backoff delay: 2^(attempt-1) * BASE_DELAY
    DELAY=$((BASE_DELAY * (1 << (attempt - 1))))
    echo "⏳ Retrying in ${DELAY}s..."
    echo ""
    sleep $DELAY
  else
    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "🔴 Max retries exhausted - build failed after $MAX_RETRIES attempts"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo ""
    echo "Possible causes:"
    echo "  - Network issues (npm/apt package downloads)"
    echo "  - Docker daemon errors"
    echo "  - Insufficient disk space"
    echo ""
    echo "Try:"
    echo "  - Check network connectivity"
    echo "  - Run: docker system prune -a (free up space)"
    echo "  - Check Docker logs: journalctl -u docker"
    echo ""
    exit 1
  fi
done
