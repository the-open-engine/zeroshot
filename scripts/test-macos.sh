#!/usr/bin/env bash
set -euxo pipefail

# Run from the repository root after building the embedded UI and installing pinned Restic.

# Parallel terminals
cargo test --locked --package zeroshot --features ui --test full_v1_reducer --test full_v1_reducer_boundaries
cargo test --locked --package zeroshot --features ui --lib -- \
  parallel_branch_terminals_cancel_and_drain_siblings_before_finishing \
  built_in_templates_have_no_terminals_inside_parallel_branches \
  parallel_terminal_ui_fixture_matches_canonical_control_replay

# Provider transcripts
cargo test --locked --package zeroshot --lib --features ui -- \
  native_v2_claude::transcript::tests \
  native_v2_codex::output::tests \
  native_v2_pi::transcript::tests

# Context repositories and environment
cargo test --locked --package zeroshot --lib --features ui -- \
  native_v2_admission::tests::context native_v2_admission::tests::workspaces caller_git_config context_git_trust context_path_is_reserved
cargo test --locked --package zeroshot --lib --features ui -- \
  active_memory::postmortem::tests::initialization
cargo test --locked --package openengine-cluster-protocol --test native_v2_run --test native_v2_observation
cargo test --locked --package openengine-cluster-protocol --lib native_v2_run::environment::tests

# Ledger storage failures
cargo test --locked --package zeroshot --lib --features ui -- \
  v2_run_ledger::sqlite::storage_failure_tests \
  native_v2_platform::tests::runtime_failure

# Process provenance
cargo test --locked --package zeroshot --lib --features ui -- execution::process::authority

# Forge network and access
cargo test --locked --package zeroshot --lib --features ui -- \
  source_code_provider::adapters::core::network::tests \
  source_code_provider::adapters::core::github::access::tests \
  source_code_provider::adapters::core::github::api::tests \
  source_code_provider::adapters::core::gitlab::tests::network_tests \
  native_v2_local::token_file::tests::caller_network_snapshot \
  native_v2_delivery::command::tests \
  native_v2_delivery::transport_http::tests \
  native_v2_delivery::tests::transport_matrix

# Controller sockets and APFS workspace clones
ZEROSHOT_REQUIRE_APFS_CLONE=1 cargo test --locked --package zeroshot --lib --features ui -- \
  native_v2_portable_controller::transport::unix::tests \
  native_v2_supervisor::checkpoints::filesystem::tests::macos_ \
  native_v2_supervisor::checkpoints::filesystem::tests::cancellation_after_metadata_leaves_only_the_prior_snapshot

# Target files, including early authentication and framing refusals
cargo test --locked --package openengine-cluster-protocol --test target_files
cargo test --locked --package zeroshot --features ui --lib -- \
  native_v2_files::tests native_v2_target_authority::tests::files \
  native_v2_target_authority::tests::requests \
  native_v2_supervisor::checkpoints::tests::git_checkpoints_restore_mutable_inputs_without_staging_them \
  native_v2_supervisor::checkpoints::tests::unborn_git_checkpoint_restores_without_a_stop_receiver \
  execution::platform::tests::private_state_round_trips_and_replaces_an_existing_file
cargo test --locked --package zeroshot --features ui --bin zeroshot -- \
  native_v2_target::serve::tests::direct_serve_surfaces_hosting_preparation_failures --exact
cargo test --locked --package zeroshot --features ui --test native_v2_files

# Workspace export
cargo test --locked --package zeroshot --features ui --test native_v2_export_workspace
cargo test --locked --package zeroshot --features ui --lib native_v2_cli::execution::export_workspace::tests

# Unhandled worker errors
cargo test --locked --package zeroshot --features ui --test full_v1_reducer -- unhandled:: trace::unchecked
cargo test --locked --package zeroshot --features ui --test native_v2_cli_portable \
  unchecked_worker_error_fails_foreground_status_and_logs
cargo test --locked --package zeroshot --features ui --test native_v2_cli_local \
  fixture::steering::steer_selected_older_checkpoint_inherits_only_receipts_in_saved_conversation
cargo test --locked --package zeroshot --features ui --lib -- \
  native_v2_cli::templates::tests::unhandled native_v2_supervisor::tests::failure_logs v2_run_ledger::tests::failure_context

# Admission and node input sizes
cargo test --locked --package zeroshot --lib --features ui -- \
  input_sizes input_size submission_size oversized_submission admitted_run_size node_input_size
cargo test --locked --package zeroshot --features ui --test native_v2_cli_portable -- \
  oversized_submissions oversized_node_input

# Checkpoint forge access, discard, diagnostics and capture logs
cargo test --locked --package zeroshot --features ui --test native_v2_local_delivery_gh_search_path \
  checkpoint_resume_checks_the_restored_gh_once_before_agents_start
cargo test --locked --package zeroshot --lib --features ui -- \
  native_v2_cli::local::backend::discard_tests workspace_discard_rejects_redirected_roots interrupted_delivery_rechecks
cargo test --locked --package zeroshot --features ui --test native_v2_cli_portable discard_workspace_tests
cargo test --locked --package zeroshot --lib --features ui -- \
  native_v2_supervisor::checkpoints::restic::tests native_v2_supervisor::tests::checkpoint_tests
cargo test --locked --package zeroshot --features ui --test native_v2_cli_local fixture::restic_diagnostics

# Controller shutdown and tool activity
cargo test --locked --package zeroshot --features ui --test native_v2_cli_portable unix_controller
cargo test --locked --package zeroshot --features ui --test native_v2_cli_portable tool_activity
cargo test --locked --package zeroshot --features ui --lib -- \
  native_v2_portable_controller::process::shutdown_tests \
  managed_cleanup_distinguishes_explicit_stop_from_recoverable_shutdown

# Read-only controller inspection and local startup
cargo test --locked --package zeroshot --features ui --lib -- \
  native_v2_portable_controller::availability::tests native_v2_cli::local::inspection::tests v2_run_ledger::sqlite::readonly::tests
cargo test --locked --package zeroshot --features ui --test native_v2_cli_portable inspection
cargo test --locked --package zeroshot --lib --features ui native_v2_cli::local::
cargo test --locked --package zeroshot --features ui --test native_v2_cli_portable startup::

# Profile and target registry store symlinks
cargo test --locked --package zeroshot --features ui --lib native_v2_cli::profiles::tests
cargo test --locked --package zeroshot --features ui --bin zeroshot native_v2_target::tests::management

# Resume checkpoint policy and network authority
cargo test --locked --package zeroshot --lib --features ui -- \
  native_v2_target_authority::tests::resume_refusals native_v2_target_authority::tests::checkpoint_opt_out
cargo test --locked --package zeroshot --features ui --test native_v2_cli_local \
  networks_cleared_agent_labels_cannot_use_human_cli_or_rpc_across_runs

# Explicit request/final publications and filepath response correction
cargo test --locked --package zeroshot --features ui --lib -- \
  native_v2_supervisor::tests::artifacts native_v2_codex::tests::filepath \
  native_v2_platform::tests::cleanup v2_run_ledger::sqlite::readonly::tests::timing \
  native_v2_runner::response native_v2_request::tests \
  native_v2_cli::execution::request_tests \
  native_v2_observability::tests::publications \
  profile_ui::runs::tests::status::request_attachments \
  native_v2_admission::tests::requests::artifact \
  native_v2_admission::profile_expansion::wire_contract::profile_linking
cargo test --locked --package openengine-cluster-protocol --test payload_subtyping --test canonical
