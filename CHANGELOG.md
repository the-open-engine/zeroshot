## [1.1.3](https://github.com/covibes/zeroshot/compare/v1.1.2...v1.1.3) (2025-12-28)


### Bug Fixes

* **publish:** remove tests from prepublishOnly to prevent double execution ([3e11e71](https://github.com/covibes/zeroshot/commit/3e11e71cb722f835634d21f80fee79ea3c29b031))

## [1.1.2](https://github.com/covibes/zeroshot/compare/v1.1.1...v1.1.2) (2025-12-28)


### Bug Fixes

* **ci:** resolve ESLint violations and status-footer test failures ([0d794f9](https://github.com/covibes/zeroshot/commit/0d794f98aa10d2492d8ab0af516bb1e5abee0566))
* **isolation:** handle missing/directory .gitconfig in CI environments ([3d754e4](https://github.com/covibes/zeroshot/commit/3d754e4a02d40e2fd902d97d17a6532ba247f780))
* **workflow:** extract tarball filename correctly from npm pack output ([3cf48a3](https://github.com/covibes/zeroshot/commit/3cf48a3ddf4f1938916c7ed5a2be1796003a988f))

## [1.1.1](https://github.com/covibes/zeroshot/compare/v1.1.0...v1.1.1) (2025-12-28)


### Bug Fixes

* **lint:** resolve require-await and unused-imports errors ([852c8a0](https://github.com/covibes/zeroshot/commit/852c8a0e9076eb5403105c6f319e66e53c27fd6d))

# [1.1.0](https://github.com/covibes/zeroshot/compare/v1.0.2...v1.1.0) (2025-12-28)


### Bug Fixes

* **docker:** use repo root as build context for Dockerfile ([c1d6719](https://github.com/covibes/zeroshot/commit/c1d6719eb43787ba62e5f69663eb4e5bd1aeb492))
* **lint:** remove unused import and fix undefined variable in test ([41c9965](https://github.com/covibes/zeroshot/commit/41c9965eb84d2b8c22eaaf8e1d65a5f41c7b1e44))


### Features

* **isolation:** use zeroshot task infrastructure inside containers ([922f30d](https://github.com/covibes/zeroshot/commit/922f30d5ddd8c4d87cac375fd97025f402e7c43e))
* **monitoring:** add live status footer with CPU/memory metrics ([2df3de0](https://github.com/covibes/zeroshot/commit/2df3de0a1fe9573961b596da9e78a159f3c33086))
* **validators:** add zero-tolerance rejection rules for incomplete code ([308aef8](https://github.com/covibes/zeroshot/commit/308aef8b5ee2e3ff05e336ee810b842492183b2e))
* **validators:** strengthen with senior engineering principles ([d83f666](https://github.com/covibes/zeroshot/commit/d83f6668a145e36bd7d807d9821e8631a3a1cc18))

## [1.0.2](https://github.com/covibes/zeroshot/compare/v1.0.1...v1.0.2) (2025-12-27)


### Bug Fixes

* include task-lib in npm package ([37602fb](https://github.com/covibes/zeroshot/commit/37602fb3f1f6cd735d8db232be5829dc342b815d))

## [1.0.1](https://github.com/covibes/zeroshot/compare/v1.0.0...v1.0.1) (2025-12-27)


### Bug Fixes

* **ci:** checkout latest main to prevent stale SHA race condition ([dd302ba](https://github.com/covibes/zeroshot/commit/dd302ba8e0755cea6835cfae3286b3aa51e2f92a))
* trigger npm publish ([6aa6708](https://github.com/covibes/zeroshot/commit/6aa6708dca0e55299ba5d1be9eb54410731a7da0))

# 1.0.0 (2025-12-27)


### Bug Fixes

* **ci:** update codecov to v5 and add continue-on-error ([53de603](https://github.com/covibes/zeroshot/commit/53de603d008764c31dc158a3f2702128d6cf8bc4))
* **ci:** use Node.js 22 for semantic-release compatibility ([#9](https://github.com/covibes/zeroshot/issues/9)) ([0387c7d](https://github.com/covibes/zeroshot/commit/0387c7dcf5211b8632cf5c19a5516ad119c69a59))
* disable checkJs to fix CI typecheck failures ([cabe14c](https://github.com/covibes/zeroshot/commit/cabe14c21e8827b26423aa1b5339cb4056f0f8a5))
* **lint:** add missing eslint-config-prettier + fix no-control-regex ([d26e1ba](https://github.com/covibes/zeroshot/commit/d26e1ba404a85c96519d2945501dfa4b09505190))
* mark task-lib as ES module for Node 18 compatibility ([44fea80](https://github.com/covibes/zeroshot/commit/44fea80bd4d28877786eb140d9a9d63ac9f609ee))
* prevent agents from asking questions in non-interactive mode ([#8](https://github.com/covibes/zeroshot/issues/8)) ([458ed29](https://github.com/covibes/zeroshot/commit/458ed299aefa2790fcc951dd0efcd9d347c485ce))
* **resume:** find last workflow trigger instead of arbitrary last 5 messages ([497c24f](https://github.com/covibes/zeroshot/commit/497c24f4bd0b8c0be168167965520600b82a3f2a))
* **test:** correct npm install retry timing assertion ([36222d6](https://github.com/covibes/zeroshot/commit/36222d69920fc1aed012002c3846cf9f7d9e6392))


### Features

* **validator:** make validator-tester repo-calibrated and intelligent ([#5](https://github.com/covibes/zeroshot/issues/5)) ([3bccad2](https://github.com/covibes/zeroshot/commit/3bccad2ab32130efb897864de2a31d10c1f1842c))
* **validators:** enforce test quality with antipattern detection ([#2](https://github.com/covibes/zeroshot/issues/2)) ([9b4f912](https://github.com/covibes/zeroshot/commit/9b4f91200f4429acbce300f2c049d1d23191e768))

# 1.0.0 (2025-12-27)


### Bug Fixes

* **ci:** update codecov to v5 and add continue-on-error ([53de603](https://github.com/covibes/zeroshot/commit/53de603d008764c31dc158a3f2702128d6cf8bc4))
* **ci:** use Node.js 22 for semantic-release compatibility ([#9](https://github.com/covibes/zeroshot/issues/9)) ([0387c7d](https://github.com/covibes/zeroshot/commit/0387c7dcf5211b8632cf5c19a5516ad119c69a59))
* disable checkJs to fix CI typecheck failures ([cabe14c](https://github.com/covibes/zeroshot/commit/cabe14c21e8827b26423aa1b5339cb4056f0f8a5))
* **lint:** add missing eslint-config-prettier + fix no-control-regex ([d26e1ba](https://github.com/covibes/zeroshot/commit/d26e1ba404a85c96519d2945501dfa4b09505190))
* mark task-lib as ES module for Node 18 compatibility ([44fea80](https://github.com/covibes/zeroshot/commit/44fea80bd4d28877786eb140d9a9d63ac9f609ee))
* prevent agents from asking questions in non-interactive mode ([#8](https://github.com/covibes/zeroshot/issues/8)) ([458ed29](https://github.com/covibes/zeroshot/commit/458ed299aefa2790fcc951dd0efcd9d347c485ce))
* **resume:** find last workflow trigger instead of arbitrary last 5 messages ([497c24f](https://github.com/covibes/zeroshot/commit/497c24f4bd0b8c0be168167965520600b82a3f2a))
* **test:** correct npm install retry timing assertion ([36222d6](https://github.com/covibes/zeroshot/commit/36222d69920fc1aed012002c3846cf9f7d9e6392))


### Features

* **validator:** make validator-tester repo-calibrated and intelligent ([#5](https://github.com/covibes/zeroshot/issues/5)) ([3bccad2](https://github.com/covibes/zeroshot/commit/3bccad2ab32130efb897864de2a31d10c1f1842c))
* **validators:** enforce test quality with antipattern detection ([#2](https://github.com/covibes/zeroshot/issues/2)) ([9b4f912](https://github.com/covibes/zeroshot/commit/9b4f91200f4429acbce300f2c049d1d23191e768))

# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2025-12-25

### Added

#### Core Architecture

- Multi-agent coordination engine with message-passing primitives
- SQLite-backed immutable ledger for crash recovery and state persistence
- Pub/sub message bus with topic-based routing and WebSocket support
- JavaScript-based logic engine for trigger evaluation with sandboxed execution
- Agent lifecycle management with state machine (idle → evaluating → executing)
- Dynamic agent spawning via CLUSTER_OPERATIONS messages

#### Agent System

- AgentWrapper for managing Claude CLI process lifecycle
- Context building from ledger with configurable strategies
- Hook system for onComplete, onError, and onStart actions
- Output streaming via message bus with real-time updates
- Liveness detection to identify stuck agents
- Resume capability for failed tasks with error context
- Dynamic model selection based on iteration count and complexity
- Support for both static and parameterized model configurations

#### CLI Commands

- `zeroshot run` - Start multi-agent cluster from GitHub issue or text
- `zeroshot auto` - Full automation with Docker isolation and auto-merge PR
- `zeroshot list` - View all running clusters and tasks
- `zeroshot status` - Get detailed cluster status with zombie detection
- `zeroshot logs` - Follow cluster output in real-time
- `zeroshot resume` - Continue from crashed or stopped clusters
- `zeroshot stop` - Graceful cluster shutdown
- `zeroshot kill` - Force stop running cluster
- `zeroshot clear` - Remove all stopped clusters
- `zeroshot export` - Export conversation as JSON or Markdown
- `zeroshot watch` - Interactive TUI dashboard (htop-style)
- `zeroshot agents` - View available agent definitions
- `zeroshot settings` - Manage global settings
- Shell completion support via omelette

#### Docker Isolation

- IsolationManager for container lifecycle management
- Fresh git repository initialization in isolated containers
- Credential mounting for Claude CLI, AWS, GitHub, Kubernetes
- Docker-in-Docker support for e2e tests
- Automatic npm dependency installation in containers
- Terraform state preservation across container cleanup
- Git worktree support (alternative to full copy)

#### Workflow Templates

- Conductor system with 2D classification (Complexity × TaskType)
- Four base templates: single-worker, worker-validator, debug-workflow, full-workflow
- Parameterized template resolution with TemplateResolver
- Dynamic agent spawning based on task analysis
- Model tier selection: Haiku (TRIVIAL), Sonnet (SIMPLE/STANDARD), Opus (CRITICAL)
- Validator scaling: 0-5 validators based on complexity
- Adversarial tester for STANDARD/CRITICAL tasks

#### GitHub Integration

- Issue fetching with automatic URL parsing
- GitHub CLI (gh) integration for PR creation
- Auto-merge support via git-pusher agent
- Token authentication with hosts.yml fallback

#### TUI Dashboard

- Real-time cluster monitoring with blessed/blessed-contrib
- Cluster list with state, agent count, and message count
- Message viewer with topic filtering
- Agent status display with iteration tracking
- Log viewer with search and navigation
- System resource monitoring (CPU, memory)
- Responsive layout with keyboard navigation

#### Developer Tools

- Config validator with strict mode and warning detection
- ESLint configuration with unused imports detection
- TypeScript type checking with JSDoc annotations
- Mocha test framework with comprehensive test coverage
- Dead code detection with ts-prune, unimported, depcheck
- Proper lockfile support for concurrent file access

#### Safety Features

- PreToolUse hook to block AskUserQuestion in non-interactive mode
- Explicit prompts for autonomous decision-making
- Git safety enforcement (no destructive operations)
- Zombie cluster detection for orphaned processes
- Retry logic with exponential backoff for network operations
- File locking for concurrent orchestrator instances

### Security

- Sandboxed JavaScript execution for trigger logic
- Frozen prototypes in VM context to prevent pollution
- Read-only mounts for credentials in Docker containers
- Docker group GID detection for socket access control
- Timeout enforcement for logic scripts (1 second limit)

## [0.0.0] - Development

Initial development phase before first release.

[Unreleased]: https://github.com/covibes/zeroshot/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/covibes/zeroshot/releases/tag/v0.1.0
