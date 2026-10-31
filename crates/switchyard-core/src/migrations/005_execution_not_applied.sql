-- Preserve v4 receipts while adding a known-not-applied terminal state.
CREATE TABLE execution_effects_v5 (
    operation_id TEXT PRIMARY KEY,
    mission_id TEXT NOT NULL REFERENCES missions(id),
    attempt_id TEXT NOT NULL REFERENCES attempts(id),
    session_id TEXT NOT NULL,
    payload_hash TEXT NOT NULL,
    request_json TEXT NOT NULL,
    started_generation INTEGER NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('prepared', 'completed', 'uncertain', 'not_applied')),
    receipt_json TEXT,
    created_at INTEGER NOT NULL,
    CHECK ((state = 'completed') = (receipt_json IS NOT NULL))
);
INSERT INTO execution_effects_v5 SELECT * FROM execution_effects;
DROP TABLE execution_effects;
ALTER TABLE execution_effects_v5 RENAME TO execution_effects;
CREATE INDEX execution_effects_mission ON execution_effects(mission_id, state);
