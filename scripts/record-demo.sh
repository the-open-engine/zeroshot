#!/bin/bash
# record-demo.sh - Create ephemeral project, run zeroshot demo, record it, cleanup
#
# Usage:
#   ./scripts/record-demo.sh              # Interactive mode (you run zeroshot manually)
#   ./scripts/record-demo.sh --record     # Record with asciinema automatically
#
# Output: zeroshot-demo.cast (asciinema recording)
# Convert to gif: agg zeroshot-demo.cast zeroshot-demo.gif --idle-time-limit 2

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DEMO_DIR=""
RECORD_MODE=false
FORCE_MODE=false
LOCK_FILE="/tmp/zeroshot-demo-recording.lock"
CAST_FILE="$REPO_ROOT/zeroshot-demo.cast"

# Check for existing recording session
check_existing_session() {
    # Check lock file
    if [[ -f "$LOCK_FILE" ]]; then
        local old_pid
        old_pid=$(cat "$LOCK_FILE" 2>/dev/null || echo "")
        if [[ -n "$old_pid" ]] && kill -0 "$old_pid" 2>/dev/null; then
            echo "ERROR: Recording already in progress (PID $old_pid)"
            echo "Kill it with: kill $old_pid"
            exit 1
        fi
        # Stale lock file
        rm -f "$LOCK_FILE"
    fi

    # Check for any asciinema processes recording to our file
    local existing_asciinema
    existing_asciinema=$(pgrep -f "asciinema.*zeroshot-demo.cast" 2>/dev/null || true)
    if [[ -n "$existing_asciinema" ]]; then
        echo "ERROR: Existing asciinema process(es) found: $existing_asciinema"
        echo "Kill them with: pkill -f 'asciinema.*zeroshot-demo.cast'"
        exit 1
    fi

    # Check for any running zeroshot clusters
    local existing_zeroshot
    existing_zeroshot=$(pgrep -f "zeroshot.*rate limiting" 2>/dev/null || true)
    if [[ -n "$existing_zeroshot" ]]; then
        echo "ERROR: Existing zeroshot process(es) found: $existing_zeroshot"
        echo "Kill them with: pkill -f 'zeroshot.*rate limiting'"
        exit 1
    fi
}

# Parse args
while [[ $# -gt 0 ]]; do
    case "$1" in
        --record) RECORD_MODE=true; shift ;;
        --force|-f) FORCE_MODE=true; shift ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

# Cleanup on exit
cleanup() {
    # Remove lock file
    rm -f "$LOCK_FILE"

    if [[ -n "$DEMO_DIR" && -d "$DEMO_DIR" ]]; then
        echo ""
        echo "Cleaning up $DEMO_DIR..."
        rm -rf "$DEMO_DIR"
        echo "Done."
    fi
}
trap cleanup EXIT

# Check for conflicts before doing anything
check_existing_session

# Create temp project
DEMO_DIR=$(mktemp -d -t zeroshot-demo-XXXXXX)
echo "Creating demo project in $DEMO_DIR"

cd "$DEMO_DIR"

# Initialize package.json
cat > package.json << 'EOF'
{
  "name": "demo-api",
  "version": "1.0.0",
  "type": "commonjs",
  "scripts": {
    "dev": "ts-node src/index.ts",
    "build": "tsc",
    "start": "node dist/index.js"
  }
}
EOF

# Install dependencies (quiet)
echo "Installing dependencies..."
npm install --silent express typescript ts-node @types/express @types/node

# Create tsconfig
cat > tsconfig.json << 'EOF'
{
  "compilerOptions": {
    "target": "ES2020",
    "module": "commonjs",
    "strict": true,
    "esModuleInterop": true,
    "outDir": "dist",
    "rootDir": "src"
  },
  "include": ["src/**/*"]
}
EOF

# Create minimal server with users "database"
mkdir -p src

cat > src/db.ts << 'EOF'
// Simple in-memory database
export interface User {
  id: number;
  username: string;
  email: string;
  password_hash: string;
  avatar: string;
  created_at: Date;
}

export const users: User[] = [
  {
    id: 1,
    username: "alice",
    email: "alice@example.com",
    password_hash: "$2b$10$X7VYKzPQ...",
    avatar: "https://example.com/alice.jpg",
    created_at: new Date("2024-01-15"),
  },
  {
    id: 2,
    username: "bob",
    email: "bob@example.com",
    password_hash: "$2b$10$Y8WZLaQR...",
    avatar: "https://example.com/bob.jpg",
    created_at: new Date("2024-02-20"),
  },
];
EOF

cat > src/index.ts << 'EOF'
import express from "express";

const app = express();
const PORT = 3000;

app.use(express.json());

// Health check
app.get("/health", (_, res) => {
  res.json({ status: "ok" });
});

app.listen(PORT, () => {
  console.log(`Server running on http://localhost:${PORT}`);
});
EOF

# Initialize git repo
git init --quiet
git add -A
git commit --quiet -m "Initial commit: Express API with users database"

echo ""
echo "=========================================="
echo "Demo project ready!"
echo "=========================================="
echo ""
echo "Directory: $DEMO_DIR"
echo ""
echo "Suggested demo task:"
echo "  zeroshot 'Add PUT /users/:id endpoint to update user profile'"
echo ""

if [[ "$RECORD_MODE" == "true" ]]; then
    echo "Recording with asciinema..."
    echo ""

    # Check asciinema is installed
    if ! command -v asciinema &> /dev/null; then
        echo "Error: asciinema not installed. Run: pip install asciinema"
        exit 1
    fi

    # Warn if cast file exists (skip with --force)
    if [[ -f "$CAST_FILE" ]] && [[ "$FORCE_MODE" != "true" ]]; then
        echo "WARNING: $CAST_FILE already exists!"
        echo "Previous recording will be OVERWRITTEN."
        echo ""
        read -p "Continue? [y/N] " -n 1 -r
        echo ""
        if [[ ! $REPLY =~ ^[Yy]$ ]]; then
            echo "Aborted."
            exit 1
        fi
    fi

    # Create lock file with our PID
    echo $$ > "$LOCK_FILE"

    # Record the session
    # CRITICAL: Use bash -l -c to get full PATH (includes npm global binaries)
    ZEROSHOT_PATH=$(which zeroshot)
    TASK="Add rate limiting middleware: sliding window algorithm (not fixed window), per-IP tracking with in-memory store and automatic TTL cleanup to prevent memory leaks, configurable limits per endpoint. Return 429 Too Many Requests with Retry-After header (seconds until reset) and X-RateLimit-Remaining header on ALL responses. Must handle both IPv4 and IPv6, normalizing IPv6 to consistent format."

    # SIGNAL ISOLATION: Use setsid to create new session, completely immune to terminal signals
    # The recording process will NOT be killed by Ctrl+C or signals to this script
    # We wait for it explicitly and only cleanup AFTER it naturally completes
    echo "Starting recording in isolated session (immune to Ctrl+C)..."
    echo "To kill it manually: pkill -f 'asciinema.*zeroshot-demo.cast'"
    echo ""

    # Disable cleanup trap during recording - we'll handle it manually
    trap - EXIT

    # Start asciinema in new session (setsid) so it's immune to our signals
    # Save the session leader PID so we can wait for it
    setsid bash -c "
        # Ignore all signals - this recording WILL NOT DIE
        trap '' INT TERM HUP QUIT

        cd '$DEMO_DIR'
        asciinema rec \
            --overwrite \
            --title 'Zeroshot Demo' \
            --command \"bash -l -c '$ZEROSHOT_PATH \\\"$TASK\\\"'\" \
            '$CAST_FILE'
    " &
    RECORDING_PID=$!

    # Update lock file with the actual recording PID
    echo $RECORDING_PID > "$LOCK_FILE"

    echo "Recording started (session PID: $RECORDING_PID)"
    echo "Waiting for recording to complete..."
    echo ""

    # Wait for recording to finish - this script will NOT kill it on Ctrl+C
    # Ignore signals while waiting
    trap '' INT TERM HUP
    wait $RECORDING_PID 2>/dev/null || true

    # Recording finished naturally - now cleanup
    echo ""
    echo "Recording saved to: $CAST_FILE"
    echo ""
    echo "Convert to gif with:"
    echo "  agg $CAST_FILE $REPO_ROOT/zeroshot-demo.gif --idle-time-limit 2"

    # Now do cleanup
    rm -f "$LOCK_FILE"
    if [[ -n "$DEMO_DIR" && -d "$DEMO_DIR" ]]; then
        echo ""
        echo "Cleaning up $DEMO_DIR..."
        rm -rf "$DEMO_DIR"
        echo "Done."
    fi
else
    echo "Interactive mode - run zeroshot manually:"
    echo ""
    echo "  cd $DEMO_DIR"
    echo "  zeroshot 'Add PUT /users/:id endpoint to update user profile'"
    echo ""
    echo "Press Enter when done to cleanup..."
    read -r
fi
