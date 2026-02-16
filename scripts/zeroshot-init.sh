#!/usr/bin/env bash
#
# zeroshot-init.sh — One-time project setup for quality gate
#
# Usage: zeroshot-init.sh [/path/to/repo]
#
# Generates .zeroshot-quality using an AI CLI to analyze the project.

set -euo pipefail

REPO_DIR="${1:-.}"
QUALITY_FILE="$REPO_DIR/.zeroshot-quality"

if [ -f "$QUALITY_FILE" ]; then
  echo "✅ $QUALITY_FILE already exists:"
  cat "$QUALITY_FILE"
  exit 0
fi

# Detect available AI CLI
AI_CLI=""
if command -v claude &>/dev/null; then
  AI_CLI="claude"
elif command -v codex &>/dev/null; then
  AI_CLI="codex"
elif command -v gemini &>/dev/null; then
  AI_CLI="gemini"
fi

PROMPT="Analyze this project and output a single shell command that runs all available quality checks (tests, linting, type checking). Output ONLY the command, no markdown, no explanation."

if [ -n "$AI_CLI" ]; then
  echo "🔍 Using $AI_CLI to analyze project..."
  COMMAND=$($AI_CLI --print "$PROMPT" 2>/dev/null || echo "")

  if [ -z "$COMMAND" ]; then
    echo "⚠️  AI CLI returned empty output. Using fallback detection."
    AI_CLI=""
  fi
fi

# Fallback: detect from project files
if [ -z "$AI_CLI" ]; then
  echo "🔍 No AI CLI found. Detecting quality checks from project files..."
  PARTS=()

  # Helper: check if a file contains a string (case-insensitive grep)
  has() { grep -qi "$2" "$REPO_DIR/$1" 2>/dev/null; }
  # Helper: check if a file exists in the repo
  exists() { [ -f "$REPO_DIR/$1" ]; }
  # Helper: check if any file matching a glob exists
  has_glob() { compgen -G "$1" >/dev/null 2>&1; }

  # Each ecosystem block is independent (not elif) so multi-ecosystem
  # projects (e.g. Laravel with PHP backend + Node frontend) get full coverage.
  DETECTED=false

  # ─── PHP ─────────────────────────────────────────────────────────
  if exists composer.json; then
    DETECTED=true
    if exists vendor/bin/phpstan; then
      PARTS+=("vendor/bin/phpstan analyse")
    fi
    if has composer.json 'laravel/framework'; then
      PARTS+=("php artisan test")
    elif has composer.json '"symfony/"'; then
      PARTS+=("php bin/phpunit")
    elif exists vendor/bin/phpunit; then
      PARTS+=("vendor/bin/phpunit")
    fi
  fi

  # ─── Ruby ────────────────────────────────────────────────────────
  if exists Gemfile; then
    DETECTED=true
    if has Gemfile 'rubocop'; then
      PARTS+=("bundle exec rubocop")
    fi
    if exists bin/rails || has Gemfile 'rails'; then
      PARTS+=("bin/rails test")
    elif has Gemfile 'rspec'; then
      PARTS+=("bundle exec rspec")
    else
      PARTS+=("bundle exec rake test")
    fi
  fi

  # ─── Python ──────────────────────────────────────────────────────
  if exists pyproject.toml || exists requirements.txt || exists setup.py || exists Pipfile || exists poetry.lock; then
    DETECTED=true
    # Linting
    if exists pyproject.toml && has pyproject.toml '\[tool\.ruff\]'; then
      PARTS+=("ruff check .")
    elif has_glob "$REPO_DIR/.flake8" || has_glob "$REPO_DIR/setup.cfg"; then
      PARTS+=("flake8 .")
    fi
    # Type checking
    if exists mypy.ini || (exists pyproject.toml && has pyproject.toml '\[tool\.mypy\]'); then
      PARTS+=("mypy .")
    fi
    # Testing
    if exists manage.py; then
      PARTS+=("python manage.py check" "python manage.py test")
    else
      PARTS+=("python -m pytest")
    fi
  fi

  # ─── Java / Kotlin ──────────────────────────────────────────────
  if exists pom.xml; then
    DETECTED=true
    if exists mvnw; then
      PARTS+=("./mvnw test")
    else
      PARTS+=("mvn test")
    fi
  elif exists build.gradle || exists build.gradle.kts; then
    DETECTED=true
    if exists gradlew; then
      PARTS+=("./gradlew test")
      if has build.gradle 'com.android.application' 2>/dev/null || has build.gradle.kts 'com.android.application' 2>/dev/null; then
        PARTS+=("./gradlew lint")
      fi
    else
      PARTS+=("gradle test")
    fi
  fi

  # ─── Node.js / TypeScript ───────────────────────────────────────
  # Checked after backend ecosystems so backend tests come first in the chain.
  if exists package.json; then
    # Detect package manager
    PKG_RUN="npm run"
    PKG_TEST="npm test"
    if exists bun.lockb || exists bunfig.toml; then
      PKG_RUN="bun run"
      PKG_TEST="bun test"
    elif exists pnpm-lock.yaml; then
      PKG_RUN="pnpm run"
      PKG_TEST="pnpm test"
    elif exists yarn.lock; then
      PKG_RUN="yarn"
      PKG_TEST="yarn test"
    fi

    NODE_PARTS=()

    # Prefer explicit npm scripts (most reliable — user configured these)
    if has package.json '"lint"'; then
      NODE_PARTS+=("$PKG_RUN lint")
    fi
    if has package.json '"typecheck"'; then
      NODE_PARTS+=("$PKG_RUN typecheck")
    fi
    if has package.json '"check"'; then
      NODE_PARTS+=("$PKG_RUN check")
    fi
    if has package.json '"test"'; then
      NODE_PARTS+=("$PKG_TEST")
    fi
    if has package.json '"build"'; then
      NODE_PARTS+=("$PKG_RUN build")
    fi

    # If no scripts found, detect from tooling config files
    if [ ${#NODE_PARTS[@]} -eq 0 ]; then
      # Linting
      if has_glob "$REPO_DIR/.eslintrc*" || has package.json '"eslint"'; then
        NODE_PARTS+=("npx eslint .")
      fi
      # Type checking
      if exists tsconfig.json; then
        NODE_PARTS+=("npx tsc --noEmit")
      fi
      # Test runners (pick one)
      if exists vitest.config.js || exists vitest.config.ts || has package.json '"vitest"'; then
        NODE_PARTS+=("npx vitest run")
      elif exists jest.config.js || exists jest.config.ts || exists jest.config.cjs || has package.json '"jest"'; then
        NODE_PARTS+=("npx jest")
      elif exists .mocharc.js || exists .mocharc.json || exists .mocharc.yaml || has package.json '"mocha"'; then
        NODE_PARTS+=("npx mocha")
      elif exists playwright.config.js || exists playwright.config.ts; then
        NODE_PARTS+=("npx playwright test")
      elif exists cypress.config.js || exists cypress.config.ts || has package.json '"cypress"'; then
        NODE_PARTS+=("npx cypress run")
      fi
      # Framework-specific build checks
      if exists next.config.js || exists next.config.mjs || exists next.config.ts || has package.json '"next"'; then
        NODE_PARTS+=("npx next build")
      elif exists nuxt.config.js || exists nuxt.config.ts || has package.json '"nuxt"'; then
        NODE_PARTS+=("npx nuxi build")
      elif exists angular.json || has package.json '"@angular/core"'; then
        NODE_PARTS+=("npx ng build")
      elif exists astro.config.mjs || exists astro.config.js || exists astro.config.ts || has package.json '"astro"'; then
        NODE_PARTS+=("npx astro build")
      elif exists vite.config.js || exists vite.config.ts || exists vite.config.mjs; then
        NODE_PARTS+=("npx vite build")
      fi
    fi

    if [ ${#NODE_PARTS[@]} -gt 0 ]; then
      DETECTED=true
      PARTS+=("${NODE_PARTS[@]}")
    fi

  elif exists deno.json || exists deno.jsonc; then
    DETECTED=true
    PARTS+=("deno lint" "deno test")
  fi

  # ─── Rust ────────────────────────────────────────────────────────
  if exists Cargo.toml; then
    DETECTED=true
    PARTS+=("cargo clippy -- -D warnings" "cargo test")
  fi

  # ─── Go ──────────────────────────────────────────────────────────
  if exists go.mod; then
    DETECTED=true
    if command -v golangci-lint &>/dev/null; then
      PARTS+=("golangci-lint run")
    fi
    PARTS+=("go vet ./..." "go test ./...")
  fi

  # ─── C# / .NET ──────────────────────────────────────────────────
  if ! $DETECTED; then
    if has_glob "$REPO_DIR/*.sln" || has_glob "$REPO_DIR/*.csproj" || exists global.json; then
      DETECTED=true
      PARTS+=("dotnet build" "dotnet test")
    fi
  fi

  # ─── Swift ───────────────────────────────────────────────────────
  if ! $DETECTED; then
    if exists Package.swift; then
      DETECTED=true
      PARTS+=("swift build" "swift test")
    fi
  fi

  # ─── Dart / Flutter ──────────────────────────────────────────────
  if ! $DETECTED; then
    if exists pubspec.yaml; then
      DETECTED=true
      if command -v flutter &>/dev/null; then
        PARTS+=("flutter analyze" "flutter test")
      else
        PARTS+=("dart analyze" "dart test")
      fi
    fi
  fi

  # ─── Scala ───────────────────────────────────────────────────────
  if ! $DETECTED; then
    if exists build.sbt; then
      DETECTED=true
      PARTS+=("sbt compile" "sbt test")
    elif exists build.sc; then
      DETECTED=true
      PARTS+=("mill compile" "mill test")
    fi
  fi

  # ─── R ───────────────────────────────────────────────────────────
  if ! $DETECTED; then
    if exists DESCRIPTION || has_glob "$REPO_DIR/*.Rproj" || exists renv.lock; then
      DETECTED=true
      PARTS+=("R CMD check .")
    fi
  fi

  # ─── Lua ─────────────────────────────────────────────────────────
  if ! $DETECTED; then
    if has_glob "$REPO_DIR/*.rockspec"; then
      DETECTED=true
      PARTS+=("luacheck .")
    fi
  fi

  # ─── C/C++ build systems ─────────────────────────────────────────
  if ! $DETECTED; then
    if exists CMakeLists.txt; then
      DETECTED=true
      PARTS+=("cmake -B build && cmake --build build && ctest --test-dir build")
    elif exists meson.build; then
      DETECTED=true
      PARTS+=("meson setup build && meson compile -C build && meson test -C build")
    elif exists WORKSPACE || exists BUILD.bazel; then
      DETECTED=true
      PARTS+=("bazel build //..." "bazel test //...")
    elif exists Makefile; then
      DETECTED=true
      if grep -q '^lint:' "$REPO_DIR/Makefile" 2>/dev/null; then
        PARTS+=("make lint")
      fi
      if grep -q '^test:' "$REPO_DIR/Makefile" 2>/dev/null; then
        PARTS+=("make test")
      elif grep -q '^check:' "$REPO_DIR/Makefile" 2>/dev/null; then
        PARTS+=("make check")
      else
        PARTS+=("make")
      fi
    fi
  fi

  if [ ${#PARTS[@]} -eq 0 ]; then
    echo "❌ Could not detect quality checks. Please create $QUALITY_FILE manually."
    echo "   Example: echo 'npm run lint && npm test' > $QUALITY_FILE"
    exit 1
  fi

  COMMAND=""
  for part in "${PARTS[@]}"; do
    if [ -n "$COMMAND" ]; then
      COMMAND="$COMMAND && $part"
    else
      COMMAND="$part"
    fi
  done
fi

echo "$COMMAND" > "$QUALITY_FILE"
echo "✅ Created $QUALITY_FILE:"
echo "   $COMMAND"

# Add to .gitignore if not already present
GITIGNORE="$REPO_DIR/.gitignore"
if [ -f "$GITIGNORE" ]; then
  if ! grep -qF '.zeroshot-quality' "$GITIGNORE"; then
    echo '.zeroshot-quality' >> "$GITIGNORE"
    echo "📝 Added .zeroshot-quality to .gitignore"
  fi
elif [ -d "$REPO_DIR/.git" ]; then
  echo '.zeroshot-quality' > "$GITIGNORE"
  echo "📝 Created .gitignore with .zeroshot-quality"
fi

echo ""
echo "Review the command above. Edit $QUALITY_FILE if needed."
