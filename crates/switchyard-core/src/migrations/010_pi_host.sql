-- Host observations are diagnostic snapshots, never agent-supplied authority.
CREATE TABLE pi_host_sessions (
    attempt_id TEXT PRIMARY KEY REFERENCES attempts(id),
    generation INTEGER NOT NULL,
    lifecycle_json TEXT NOT NULL,
    observation_json TEXT NOT NULL,
    observed_at INTEGER NOT NULL
);
