/**
 * Integrity and redaction guard for the operator-attested live-operation evidence
 * collected for issues #20–#23. The fixtures deliberately retain observations,
 * hashes, and presence markers rather than prompts, transcripts, or env values;
 * the private raw preimages are not reproduced from a clean clone.
 */
import { createHash } from 'node:crypto';
import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';

import { beforeAll, describe, expect, it } from 'vitest';

import { REPO_ROOT } from '../src/common.js';

const EVIDENCE = join(REPO_ROOT, 'adw_sdlc', 'test', 'fixtures', 'live-evidence');

function loadJson(name: string): any {
  return JSON.parse(readFileSync(join(EVIDENCE, name), 'utf8'));
}

describe('committed live evidence for issues #20–#23', () => {
  let failure: any;
  let boundary: any;
  let routing: any;
  let veto: any;
  let merge: any;
  let crossLanguage: any;

  beforeAll(() => {
    failure = loadJson('failure-drills.json');
    boundary = loadJson('secret-boundary.json');
    routing = loadJson('model-routing.json');
    veto = loadJson('tool-veto.json');
    merge = loadJson('merge-refusal.json');
    crossLanguage = loadJson('cross-language-resume.json');
  });

  it('archives timeout and native-budget fast-fails with one attempt and no nudge', () => {
    expect(failure.timeout).toMatchObject({
      run_id: 'a6b4e6dc',
      cli_exit_code: 1,
      requested_timeout_seconds: 1,
      signal: 'timeout',
      completed_phases: ['setup'],
    });
    expect(failure.timeout.error_excerpt).toContain('timed out without parseable output');
    expect(failure.timeout.no_nudge).toEqual({
      attempt_transcript_names: ['transcript.log'],
      nudge_transcript_present: false,
      metrics_present: false,
    });

    expect(failure.budget).toMatchObject({
      run_id: 'b20d9e02',
      cli_exit_code: 1,
      requested_timeout_seconds: 3600,
      requested_max_budget_usd: 0.01,
      signal: 'budget',
      completed_phases: ['setup'],
    });
    expect(failure.budget.error_excerpt).toContain('native budget cap');
    expect(failure.budget.no_nudge).toEqual({
      attempt_transcript_names: ['transcript.log'],
      nudge_transcript_present: false,
      metrics_present: false,
    });

    for (const drill of [failure.timeout, failure.budget]) {
      expect(drill.artifacts.some((artifact: any) => artifact.relative_name.endsWith('prompt.txt')))
        .toBe(true);
      expect(drill.artifacts.some((artifact: any) => artifact.relative_name.endsWith('transcript.log')))
        .toBe(true);
      expect(drill.artifacts.every((artifact: any) => artifact.relative_name !== 'transcript-2.log'))
        .toBe(true);
    }
  });

  it('records completed-phase cleanup and resume from the persisted #20 state', () => {
    const drill = failure.kill_resume;
    expect(drill.run_id).toBe('c20e5a01');
    expect(drill.interrupted_execution.cli_exit_code).toBe(130);
    expect(drill.resumed_execution.cli_exit_code).toBe(130);
    expect(drill.interrupted_execution.process_group_before_interrupt_count).toBe(4);
    expect(drill.interrupted_execution.process_group_after_interrupt_count).toBe(0);
    expect(drill.resumed_execution.process_group_before_interrupt_count).toBe(4);
    expect(drill.resumed_execution.process_group_after_interrupt_count).toBe(0);
    expect(drill.resumed_execution.resume_log_excerpt).toBe(
      '>> skipping review (already completed)',
    );
    expect(drill.interrupted_execution.state_sha256).toBe(
      drill.resumed_execution.state_sha256,
    );
    expect(drill).toMatchObject({
      persisted_state_reused: true,
      completed_phase_was_not_rerun: true,
      state_unchanged_across_resume: true,
      recorded_process_group_empty_after_each_interrupt: true,
    });
  });

  it('records an active Claude phase interrupted, cleaned up, and rerun on resume', () => {
    const drill = failure.mid_phase_kill_resume;
    expect(drill).toMatchObject({
      run_id: '57b6bfea',
      issue_number: 20,
      requested_phase: 'review',
      active_runner_cleanup_observed: true,
      persisted_incomplete_phase_was_reexecuted: true,
      recorded_process_group_empty_after_each_interrupt: true,
    });

    expect(drill.initial_execution).toMatchObject({
      cli_exit_code: 130,
      completed_phases_before_interrupt: ['setup'],
      completed_phases_after_interrupt: ['setup'],
      active_runner_process_kind: 'claude',
      active_runner_process_count: 1,
      process_group_before_interrupt_count: 6,
      process_group_after_interrupt_count: 0,
      state_unchanged_across_interrupt: true,
      tracked_worktree_clean: true,
      state_bytes: 613,
      state_sha256: 'd33420a7834395255a3c9f4faa4429aedf484659dc4d4527678078929df1deba',
      transcript_present_at_interrupt: true,
      transcript_snapshot_archived: false,
      exit_code_artifact_sha256:
        'f5bde7eb9f6c71611dc5726e8aca3eb4eba3e386da49e0a4ed5c295a90a73a0d',
      raw_cli_log_sha256:
        '3ffecaa786bb3b5e97880ba0c284529fd109d6a6eb1425d41e0812739b2488a5',
      spawn_audit_sha256:
        'c0f21e90490aa05166c546fca8b80d232134df1844b48968049e26c145954199',
      process_group_before_sha256:
        '5dc1eb06d07c2fc0420a7c31f49e11260c24ad6cc88b543ad2b4dd67daaf30ca',
      process_group_after_sha256:
        'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855',
    });
    expect(drill.initial_execution.interrupt_point).toContain(
      'real Claude subprocess was active',
    );
    expect(drill.initial_execution.observed_process_kinds_before_interrupt).toContain('claude');
    expect(drill.initial_execution.observed_process_kinds_before_interrupt).not.toContain('sleep');

    expect(drill.resumed_execution).toMatchObject({
      cli_exit_code: 130,
      completed_phases: ['setup', 'review'],
      incomplete_phase_was_reexecuted: true,
      process_group_before_final_stop_count: 6,
      process_group_after_final_stop_count: 0,
      state_unchanged_across_final_stop: true,
      tracked_worktree_clean: true,
      state_bytes: 3486,
      state_sha256: '375bd40367701870bc0d53eaf80fa97d88a5fc0d2e7b60cd24668ba4c947ff7f',
      transcript_bytes: 4096,
      transcript_sha256: '3a2ec09c16a69e03811ff9cac1ca792d22c5a288942efb49c9373bfbac5e26dc',
      raw_cli_log_sha256:
        '0740b37b6369462c78f1dbb8727a75bb55bafbfc43d0d883de634afeeecb4a04',
      exit_code_artifact_sha256:
        'f5bde7eb9f6c71611dc5726e8aca3eb4eba3e386da49e0a4ed5c295a90a73a0d',
      process_group_before_final_stop_sha256:
        '133fc86f6a9f552dd044d6f3ddd87ab1163b4d1f21748572769240dc1c1c79fd',
      process_group_after_final_stop_sha256:
        'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855',
      phase_metrics: {
        phase: 'review',
        model: 'claude-opus-4-8',
        attempts: 1,
        cost_usd: 1.1385020000000001,
        metrics_sha256:
          '222789ee588d8b72e62e1e7f744f1de9f9ed935f23388a8515db79a0fa780ad7',
      },
    });
    expect(drill.resumed_execution.observed_process_kinds_before_final_stop).toContain('sleep');
    expect(drill.resumed_execution.observed_process_kinds_before_final_stop).not.toContain('claude');
    expect(drill.resumed_execution.state_sha256).not.toBe(
      drill.initial_execution.state_sha256,
    );
    expect(drill.cost_envelope).toMatchObject({
      configured_max_budget_usd: 45,
      persisted_completed_phase_cost_usd: 1.1385020000000001,
      interrupted_attempt_cost_usd_known: false,
      persisted_completed_phase_within_configured_cap: true,
    });
    expect(drill.cost_envelope.persisted_completed_phase_cost_usd).toBeLessThan(
      drill.cost_envelope.configured_max_budget_usd,
    );
  });

  it('preserves the earlier #20 review measurement as an observed cost envelope', () => {
    const envelope = failure.kill_resume.cost_envelope;
    const observedSum = envelope.observed_phase_costs.reduce(
      (sum: number, phase: any) => sum + phase.cost_usd,
      0,
    );
    expect(envelope).toMatchObject({
      configured_max_budget_usd: 45,
      snapshot_point: 'after review persisted and before the first interrupt',
      within_configured_cap: true,
    });
    expect(envelope.observed_phase_costs.map((phase: any) => phase.phase)).toEqual(['review']);
    expect(observedSum).toBeCloseTo(0.8932829999999999, 10);
    expect(envelope.summed_total_cost_usd).toBeCloseTo(observedSum, 10);
    expect(envelope.persisted_total_cost_usd).toBeCloseTo(observedSum, 10);
    expect(envelope.persisted_total_cost_usd).toBeLessThan(envelope.configured_max_budget_usd);
  });

  it('combines the tiny cap, active-runner cleanup, and current cost envelope for #22', () => {
    const drill = failure.mid_phase_kill_resume;
    expect(failure.budget).toMatchObject({
      requested_max_budget_usd: 0.01,
      signal: 'budget',
      cli_exit_code: 1,
    });
    expect(drill).toMatchObject({
      active_runner_cleanup_observed: true,
      recorded_process_group_empty_after_each_interrupt: true,
      cost_envelope: {
        configured_max_budget_usd: 45,
        persisted_completed_phase_cost_usd: 1.1385020000000001,
        interrupted_attempt_cost_usd_known: false,
        persisted_completed_phase_within_configured_cap: true,
      },
    });
    expect(drill.initial_execution.observed_process_kinds_before_interrupt).toContain('claude');
    expect(drill.initial_execution.tracked_worktree_clean).toBe(true);
    expect(drill.resumed_execution.tracked_worktree_clean).toBe(true);
  });

  it('pairs five poisoned parent keys with a successful real spawn that observed none', () => {
    const primary = boundary.primary_paired_probe;
    const required = [
      'GH_BIN',
      'GH_TOKEN',
      'MATRIX_LIVE_AUDIT',
      'ADW_LIVE_AUDIT',
      'MX_AGENT_LIVE_AUDIT',
    ];

    expect(primary.parent_presence).toMatchObject({
      values_recorded: false,
      required_parent_key_names: required,
      present_parent_key_names: required,
      all_required_present: true,
    });
    expect(primary.spawned_executable).toMatchObject({
      real_claude_spawn: true,
      value_output_forbidden: true,
      expected_parent_poisoned_key_names: required,
      observed_denied_key_names: [],
      result: 'PASS',
    });
    expect(primary.successful_adapter_result).toMatchObject({
      cli_exit_code: 0,
      model: 'claude-haiku-4-5',
      ok: true,
      signal: 'none',
      structured_ok: true,
      session_id_present: true,
    });

    expect(boundary.audits).toHaveLength(7);
    for (const audit of boundary.audits) {
      expect(audit.real_claude_spawn).toBe(true);
      expect(audit.value_output_forbidden).toBe(true);
      expect(audit.parent_poisoned_key_names).toEqual(required);
      expect(audit.observed_denied_key_names).toEqual([]);
      expect(audit.result).toBe('PASS');
    }
  });

  it('resolves all nine Claude routes from config and has a completed live exemplar per tier', () => {
    const configPath = join(REPO_ROOT, '.adw', 'config.json');
    const configText = readFileSync(configPath, 'utf8');
    const config = JSON.parse(configText);
    const configHash = createHash('sha256').update(configText).digest('hex');

    expect(routing.source_config_sha256).toBe(configHash);
    expect(routing.routes).toHaveLength(9);
    expect(new Set(routing.routes.map((route: any) => route.phase))).toEqual(
      new Set(['classify', 'plan', 'implement', 'tests', 'resolve', 'e2e', 'review', 'patch', 'document']),
    );

    for (const route of routing.routes) {
      const tier = config.models.phaseTiers[route.phase];
      expect(route.tier).toBe(tier);
      expect(route.resolved_model).toBe(config.models.tiers[tier].claude);
      expect(routing.live_tier_exemplars[tier].model).toBe(route.resolved_model);
      expect(routing.live_tier_exemplars[tier].signal).toBe('none');
    }

    expect(routing.live_tier_exemplars.cheap).toMatchObject({
      cli_exit_code: 130,
      phase: 'classify',
      model: 'claude-haiku-4-5',
      cost_usd: 0.0668047,
      process_group_after_interrupt_count: 0,
    });
    expect(routing.live_tier_exemplars.mid).toMatchObject({
      cli_exit_code: 0,
      phase: 'tests',
      model: 'claude-sonnet-4-6',
      cost_usd: 0.1685902,
      session_id_present: true,
    });
    expect(routing.live_tier_exemplars.capable).toMatchObject({
      cli_exit_code: 130,
      phase: 'review',
      model: 'claude-opus-4-8',
      cost_usd: 0.8932829999999999,
    });
  });

  it('records the production git/gh veto firing while redacting tool input', () => {
    expect(veto.probe).toMatchObject({
      cli_exit_code: 0,
      real_claude_spawn: true,
      model: 'claude-haiku-4-5',
      signal: 'none',
      structured_output: { blocked: true },
      session_id_present: true,
    });
    expect(veto.production_hook_event).toMatchObject({
      tool_name: 'Bash',
      category: 'git-gh-veto',
      decision: 'deny',
      input_recorded: false,
    });
    expect(veto.orchestrator_side_effect_check).toEqual({
      side_effect_kind: 'unique git tag',
      matching_tag_count_before: 0,
      matching_tag_count_after: 0,
      side_effect_present_before: false,
      side_effect_present_after: false,
      tag_text_archived: false,
      command_text_archived: false,
    });
    expect(veto.redaction).toEqual({
      raw_tool_input_archived: false,
      command_text_archived: false,
      hook_records_decision_metadata_only: true,
    });
  });

  it('records unattended merge refusal and leaves PR #66 open, green, and unmerged', () => {
    expect(merge).toMatchObject({ reused_run_id: 'c20e5a01' });
    expect(merge.pr_creation_pass.cli_exit_code).toBe(0);
    expect(merge.pr_creation_pass.sensitive_gh_arguments_redacted_with_ellipsis).toBe(true);
    expect(merge.pr_creation_pass.log_excerpts.some((line: string) => line.endsWith('…'))).toBe(true);
    expect(merge.unattended_refusal_pass).toMatchObject({
      cli_exit_code: 1,
      yes_flag_present: false,
      assume_yes_environment_present: false,
      state_unchanged: true,
      merge_executed: false,
    });
    expect(merge.unattended_refusal_pass.error_excerpt).toContain(
      'refusing to merge unattended without --yes',
    );
    expect(merge.unattended_refusal_pass.state_before_sha256).toBe(
      merge.unattended_refusal_pass.state_after_sha256,
    );
    expect(merge.pr_after_refusal).toMatchObject({
      number: 66,
      state: 'OPEN',
      merged_at: null,
      merge_state_status: 'CLEAN',
    });
    expect(merge.pr_after_refusal.checks).toHaveLength(2);
    expect(
      merge.pr_after_refusal.checks.every(
        (check: any) => check.status === 'COMPLETED' && check.conclusion === 'SUCCESS',
      ),
    ).toBe(true);
  });

  it('pins the real Python engine commit and its skip of TypeScript live state', () => {
    expect(crossLanguage).toMatchObject({
      source_engine: 'ts',
      resume_engine: 'python',
      python_engine_commit: 'd8b3569c35eec71cb8ead13f3ebdc6d56c959a3a',
      adw_id: 'c20e5a01',
      issue: 20,
      requested_phases: ['review'],
      completed_phase_was_not_rerun: true,
      probe_exit_code: 0,
      runner_invoked: false,
      external_finalize_executed: false,
    });
    expect(crossLanguage.log_excerpts).toContain('>> skipping review (already completed)');
    expect(crossLanguage.typescript_state.completed_phases).toEqual(['setup', 'review']);
    expect(crossLanguage.python_rewritten_state.completed_phases).toEqual(['setup', 'review']);
    expect(crossLanguage.typescript_source_state_sha256).toBe(
      failure.kill_resume.interrupted_execution.state_sha256,
    );
    expect(crossLanguage.typescript_state.sha256).toBe(
      crossLanguage.typescript_source_state_sha256,
    );
    expect(crossLanguage.python_rewritten_state.dropped_additive_keys).toEqual([
      'engine',
      'runner',
      'total_cost_usd',
      'work_item',
    ]);
  });

  it('commits manifests and hashes only—never raw prompts, transcripts, paths, or secrets', () => {
    const expectedFiles = [
      'README.md',
      'cross-language-resume.json',
      'failure-drills.json',
      'merge-refusal.json',
      'model-routing.json',
      'secret-boundary.json',
      'tool-veto.json',
    ];
    const files = readdirSync(EVIDENCE).sort();
    expect(files).toEqual(expectedFiles);

    for (const name of files) {
      const content = readFileSync(join(EVIDENCE, name), 'utf8');
      expect(content).not.toMatch(/\/(?:tmp|home|Users)\//);
      expect(content).not.toMatch(/[A-Za-z]:\\/);
      expect(content).not.toMatch(/(?:gh[opusr]_|github_pat_|sk-(?:ant-|proj-)?)/i);
      expect(content).not.toMatch(/\beyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\b/);
      expect(name).not.toMatch(/prompt\.txt|transcript(?:-\d+)?\.log/);
    }

    expect(failure.sanitization).toEqual({
      full_prompts_archived: false,
      full_transcripts_archived: false,
      environment_values_archived: false,
      absolute_paths_archived: false,
    });
    expect(boundary.environment_values_archived).toBe(false);
  });
});
