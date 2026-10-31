-- Additive: preserve all v5 effect intents and receipts.
CREATE TABLE execution_resolutions (
    command_id TEXT PRIMARY KEY,
    operation_id TEXT NOT NULL UNIQUE REFERENCES execution_effects(operation_id),
    mission_id TEXT NOT NULL REFERENCES missions(id),
    request_hash TEXT NOT NULL,
    audit_json TEXT NOT NULL
);
CREATE INDEX execution_resolutions_mission ON execution_resolutions(mission_id);
