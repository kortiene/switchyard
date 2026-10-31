# Rust acceptance scenarios

Tests stay inside the core crate under `#[cfg(test)]`; no production API is exposed.
`fixtures.rs` shares the existing world, admission, notification and publication helpers.
Helpers used by one contract stay with that contract.

## C2 relocation evidence

Baseline: `b03313109fe16bd3031efdbc75d753ece28e5137`.
Discovery before and after used `cargo test --workspace --locked -- --list` on Rust 1.99.1.
Both inventories contain **52 tests**: 49 core tests and 3 CLI tests.
The 27 tests below move into contract modules; six recovery regressions keep their original names.
The other 16 core tests and all 3 CLI tests are unchanged.

All existing scenario names, assertions, failure boundaries and operation ordering remain intact.
The only test-source adjustment is `include_str!("../../Cargo.toml")` in `build_contract.rs`,
which still reads the same core manifest after the move. Fixture visibility is limited to this
test module. No production code, schema, default, workflow or public API changes.

## Qualified-name mapping

For each row, the old name is `acceptance::<scenario>` and the new name is
`acceptance::<module>::<scenario>`.

| Scenario | Module |
| --- | --- |
| `redelivery_with_a_different_payload_conflicts_and_keeps_the_command` | `admission_identity` |
| `stale_revision_mismatched_ids_and_illegal_transitions_do_not_move` | `admission_identity` |
| `operation_conflict_does_not_change_phase_and_duplicate_id_replays` | `admission_identity` |
| `steering_keeps_phase_and_a_second_mission_is_rejected` | `admission_identity` |
| `restart_after_admission_keeps_mission_attempt_and_pending_command` | `recovery_outbox` |
| `needs_attention_recovery_repair_limits_and_terminal_identity` | `recovery_outbox` |
| `failed_journal_write_is_not_acknowledged_on_retry_or_restart` | `recovery_outbox` |
| `cancelled_mission_does_not_dispatch_pending_work_and_outbox_keeps_admission_order` | `recovery_outbox` |
| `resume_enqueues_a_runtime_continuation_for_withheld_work` | `recovery_outbox` |
| `pause_resume_and_pause_during_publishing` | `pause_cancel` |
| `pause_stop_and_cancel_must_settle_before_work_resumes` | `pause_cancel` |
| `acknowledged_work_requires_a_delivered_abort_before_stop_or_cancel` | `pause_cancel` |
| `content_drift_recovers_to_verifying_and_publication_stays_reconcile_only` | `verification_approval` |
| `terminal_missions_reject_content_writes` | `verification_approval` |
| `paused_review_drift_repairs_the_resume_target` | `verification_approval` |
| `paused_review_targets_without_pass_evidence_resume_into_verifying` | `verification_approval` |
| `terminal_verification_notifications_do_not_write_evidence` | `verification_approval` |
| `fake_runtime_drives_the_state_machine_without_credentials_or_git_writes` | `publication` |
| `cancellation_during_publication_can_finish_or_discover_the_pr` | `publication` |
| `second_active_owner_is_rejected_and_inspection_does_not_take_the_lock` | `ownership` |
| `real_git_identity_is_frozen_and_a_non_repo_is_rejected` | `ownership` |
| `aliased_database_paths_share_one_owner_and_generation_is_reread` | `ownership` |
| `runtime_notifications_never_set_mission_status_directly` | `runtime_authority` |
| `incomplete_notification_frames_do_not_advance_and_errors_keep_the_queue` | `runtime_authority` |
| `operators_cannot_certify_observations_and_origin_urls_drop_userinfo` | `runtime_authority` |
| `ci_pins_the_rust_core_job_without_replacing_npm_verify` | `build_contract` |
| `core_manifest_has_no_model_or_forge_client` | `build_contract` |

The following names remain `acceptance::recovery_regressions::<scenario>`:

- `failed_pause_recovery_waits_for_abort_across_restart`
- `revised_plan_resume_ignores_prior_planning_acknowledgment`
- `second_repair_resume_ignores_prior_repair_acknowledgment`
- `explicit_recovery_reuses_pending_request_with_or_without_lost_ack`
- `recovery_reuses_pending_and_acknowledged_continuation`
- `reconciling_lost_start_ack_keeps_its_pending_abort`

## Validation

```sh
cargo test --workspace --locked -- --list
./scripts/rust-verify.sh
```

The Rust gate runs formatting, Clippy with warnings denied, and the complete workspace test suite.
These checks use FakeRuntime. They do not grant S2 GO, confinement or live execution.
