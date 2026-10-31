CREATE TABLE execution_effects (
    operation_id TEXT PRIMARY KEY,
    mission_id TEXT NOT NULL REFERENCES missions(id),
    attempt_id TEXT NOT NULL REFERENCES attempts(id),
    session_id TEXT NOT NULL,
    payload_hash TEXT NOT NULL,
    request_json TEXT NOT NULL,
    started_generation INTEGER NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('prepared', 'completed', 'uncertain')),
    receipt_json TEXT,
    created_at INTEGER NOT NULL,
    CHECK ((state = 'completed') = (receipt_json IS NOT NULL))
);
CREATE INDEX execution_effects_mission ON execution_effects(mission_id, state);
