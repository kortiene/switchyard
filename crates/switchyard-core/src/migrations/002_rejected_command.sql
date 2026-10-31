-- A definite Pi admission refusal is not an acknowledgement and not an S1
-- suppression. SQLite cannot widen a CHECK in place, so rebuild the table.
CREATE TABLE commands_v2 (
    id TEXT PRIMARY KEY,
    mission_id TEXT NOT NULL REFERENCES missions(id),
    attempt_id TEXT NOT NULL REFERENCES attempts(id),
    seq INTEGER NOT NULL UNIQUE,
    kind TEXT NOT NULL,
    payload_canonical TEXT NOT NULL,
    payload_hash TEXT NOT NULL,
    expected_revision INTEGER NOT NULL,
    resulting_revision INTEGER,
    state TEXT NOT NULL CHECK (state IN ('pending', 'acknowledged', 'reconciled', 'superseded', 'rejected')),
    request_id TEXT NOT NULL UNIQUE,
    origin TEXT NOT NULL,
    runtime_receipt_json TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

INSERT INTO commands_v2 (
    id, mission_id, attempt_id, seq, kind, payload_canonical, payload_hash,
    expected_revision, resulting_revision, state, request_id, origin,
    runtime_receipt_json, created_at, updated_at
)
SELECT
    id, mission_id, attempt_id, seq, kind, payload_canonical, payload_hash,
    expected_revision, resulting_revision, state, request_id, origin,
    runtime_receipt_json, created_at, updated_at
FROM commands;

DROP TABLE commands;
ALTER TABLE commands_v2 RENAME TO commands;
CREATE INDEX commands_mission_state ON commands(mission_id, state, seq);
