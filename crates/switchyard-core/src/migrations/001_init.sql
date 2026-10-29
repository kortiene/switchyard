CREATE TABLE host_owner (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    generation INTEGER NOT NULL CHECK (generation >= 1),
    owner_token TEXT NOT NULL,
    host_id TEXT NOT NULL,
    pid INTEGER NOT NULL,
    acquired_at INTEGER NOT NULL
);

CREATE TABLE missions (
    id TEXT PRIMARY KEY,
    revision INTEGER NOT NULL CHECK (revision >= 1),
    phase TEXT NOT NULL CHECK (phase IN (
        'draft',
        'planning',
        'awaiting_plan_approval',
        'implementing',
        'verifying',
        'awaiting_review',
        'approved',
        'publishing',
        'published',
        'rejected',
        'cancelled',
        'pausing',
        'paused',
        'cancelling',
        'needs_attention'
    )),
    objective TEXT NOT NULL,
    repo_identity_json TEXT NOT NULL,
    source_issue_json TEXT NOT NULL,
    resume_target TEXT,
    prior_phase TEXT,
    attention_reason TEXT,
    permitted_recovery_target TEXT,
    reconcile_only INTEGER NOT NULL CHECK (reconcile_only IN (0, 1)),
    content_digest TEXT NOT NULL,
    plan_digest TEXT,
    plan_body TEXT,
    reviewed_digest TEXT,
    successor_of_mission_id TEXT REFERENCES missions(id),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE tasks (
    id TEXT PRIMARY KEY,
    mission_id TEXT NOT NULL REFERENCES missions(id),
    acceptance_criteria TEXT NOT NULL,
    root INTEGER NOT NULL CHECK (root IN (0, 1)),
    frozen INTEGER NOT NULL CHECK (frozen IN (0, 1)),
    created_at INTEGER NOT NULL
);

CREATE TABLE attempts (
    id TEXT PRIMARY KEY,
    mission_id TEXT NOT NULL REFERENCES missions(id),
    task_id TEXT NOT NULL REFERENCES tasks(id),
    base_sha TEXT NOT NULL,
    session_id TEXT NOT NULL,
    runtime_config_json TEXT NOT NULL,
    policy_json TEXT NOT NULL,
    policy_hash TEXT NOT NULL,
    verifier_json TEXT NOT NULL,
    environment_json TEXT NOT NULL,
    successor_of TEXT REFERENCES attempts(id),
    active INTEGER NOT NULL CHECK (active IN (0, 1)),
    repair_count INTEGER NOT NULL CHECK (repair_count >= 0),
    max_repairs INTEGER NOT NULL CHECK (max_repairs >= 0),
    plan_revision_count INTEGER NOT NULL CHECK (plan_revision_count >= 0),
    max_plan_revisions INTEGER NOT NULL CHECK (max_plan_revisions >= 0),
    created_at INTEGER NOT NULL
);

CREATE TABLE commands (
    id TEXT PRIMARY KEY,
    mission_id TEXT NOT NULL REFERENCES missions(id),
    attempt_id TEXT NOT NULL REFERENCES attempts(id),
    kind TEXT NOT NULL,
    payload_canonical TEXT NOT NULL,
    payload_hash TEXT NOT NULL,
    expected_revision INTEGER NOT NULL,
    resulting_revision INTEGER,
    state TEXT NOT NULL CHECK (state IN ('pending', 'acknowledged', 'reconciled')),
    request_id TEXT NOT NULL UNIQUE,
    origin TEXT NOT NULL,
    runtime_receipt_json TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX commands_mission_state ON commands(mission_id, state, created_at);

CREATE TABLE operation_receipts (
    operation_id TEXT PRIMARY KEY,
    attempt_id TEXT NOT NULL REFERENCES attempts(id),
    mission_id TEXT NOT NULL REFERENCES missions(id),
    command_id TEXT,
    payload_hash TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('prepared', 'running', 'completed', 'uncertain')),
    outcome_json TEXT,
    created_at INTEGER NOT NULL
);

CREATE TABLE evidence (
    id TEXT PRIMARY KEY,
    mission_id TEXT NOT NULL REFERENCES missions(id),
    attempt_id TEXT NOT NULL REFERENCES attempts(id),
    content_hash TEXT NOT NULL,
    completeness TEXT NOT NULL CHECK (completeness IN ('complete', 'incomplete', 'truncated')),
    provenance_json TEXT NOT NULL,
    body_json TEXT NOT NULL,
    invalidated INTEGER NOT NULL CHECK (invalidated IN (0, 1)),
    created_at INTEGER NOT NULL
);

CREATE TABLE verifications (
    id TEXT PRIMARY KEY,
    mission_id TEXT NOT NULL REFERENCES missions(id),
    attempt_id TEXT NOT NULL REFERENCES attempts(id),
    evidence_id TEXT REFERENCES evidence(id),
    outcome TEXT NOT NULL CHECK (outcome IN ('pass', 'fail', 'error', 'incomplete')),
    requirements_json TEXT NOT NULL,
    content_digest TEXT NOT NULL,
    invalidated INTEGER NOT NULL CHECK (invalidated IN (0, 1)),
    created_at INTEGER NOT NULL
);

CREATE TABLE decisions (
    id TEXT PRIMARY KEY,
    mission_id TEXT NOT NULL REFERENCES missions(id),
    attempt_id TEXT NOT NULL REFERENCES attempts(id),
    actor TEXT NOT NULL,
    action TEXT NOT NULL,
    rationale TEXT NOT NULL,
    digest TEXT,
    invalidated INTEGER NOT NULL CHECK (invalidated IN (0, 1)),
    command_id TEXT,
    created_at INTEGER NOT NULL
);

CREATE TABLE deliveries (
    id TEXT PRIMARY KEY,
    mission_id TEXT NOT NULL UNIQUE REFERENCES missions(id),
    attempt_id TEXT NOT NULL REFERENCES attempts(id),
    target_branch TEXT NOT NULL,
    base_branch TEXT NOT NULL,
    approved_digest TEXT NOT NULL,
    tree_sha TEXT,
    commit_sha TEXT,
    pr_identity TEXT,
    publication_state TEXT NOT NULL CHECK (publication_state IN ('intent_recorded', 'published')),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
