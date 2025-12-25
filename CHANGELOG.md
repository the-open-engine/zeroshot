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

- Conductor system with 3D classification (Domain × Complexity × TaskType)
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
