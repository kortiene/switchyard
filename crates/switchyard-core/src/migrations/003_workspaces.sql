-- Host-owned S3-A1 workspace intent. No agent execution is enabled.
CREATE TABLE workspaces (
    id TEXT PRIMARY KEY,
    mission_id TEXT NOT NULL REFERENCES missions(id),
    attempt_id TEXT NOT NULL UNIQUE REFERENCES attempts(id),
    path TEXT NOT NULL UNIQUE,
    ledger_path TEXT NOT NULL,
    base_sha TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('reserved', 'creating', 'ready', 'uncertain')),
    root TEXT NOT NULL,
    root_identity TEXT NOT NULL,
    source TEXT NOT NULL,
    common_dir TEXT NOT NULL,
    common_identity TEXT NOT NULL,
    slot_identity TEXT,
    lease_identity TEXT,
    tree_identity TEXT,
    git_dir TEXT,
    created_generation INTEGER NOT NULL,
    verified_generation INTEGER,
    detail TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
