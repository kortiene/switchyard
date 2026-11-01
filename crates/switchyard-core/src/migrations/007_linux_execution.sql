-- Additive Linux execution evidence. v1–v6 simulated intents, receipts and
-- resolutions are preserved. Simulated receipt JSON is never parsed as a
-- Linux receipt, and the two journals cannot share an operation or resolution
-- command identity.
CREATE TABLE IF NOT EXISTS linux_execution_effects (
    operation_id TEXT PRIMARY KEY,
    mission_id TEXT NOT NULL REFERENCES missions(id),
    attempt_id TEXT NOT NULL REFERENCES attempts(id),
    session_id TEXT NOT NULL,
    payload_hash TEXT NOT NULL,
    request_json TEXT NOT NULL,
    started_generation INTEGER NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('prepared', 'entered', 'running', 'completed', 'uncertain', 'not_applied')),
    cgroup_path TEXT,
    cgroup_dev INTEGER,
    cgroup_ino INTEGER,
    boot_id TEXT,
    launcher_pid INTEGER,
    launcher_start TEXT,
    entered_at INTEGER,
    receipt_json TEXT,
    quiescence_json TEXT,
    created_at INTEGER NOT NULL,
    CHECK ((cgroup_path IS NULL) = (cgroup_dev IS NULL)),
    CHECK ((cgroup_path IS NULL) = (cgroup_ino IS NULL)),
    CHECK ((cgroup_path IS NULL) = (boot_id IS NULL)),
    CHECK (state != 'completed' OR (receipt_json IS NOT NULL AND quiescence_json IS NOT NULL)),
    CHECK (state != 'not_applied' OR (entered_at IS NULL AND receipt_json IS NULL AND quiescence_json IS NOT NULL)),
    CHECK (state != 'prepared' OR entered_at IS NULL),
    CHECK (state != 'running' OR entered_at IS NOT NULL)
);
CREATE INDEX IF NOT EXISTS linux_execution_effects_mission ON linux_execution_effects(mission_id, state);

CREATE TABLE IF NOT EXISTS linux_execution_resolutions (
    command_id TEXT PRIMARY KEY,
    operation_id TEXT NOT NULL UNIQUE REFERENCES linux_execution_effects(operation_id),
    mission_id TEXT NOT NULL REFERENCES missions(id),
    request_hash TEXT NOT NULL,
    audit_json TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS linux_execution_resolutions_mission ON linux_execution_resolutions(mission_id);

DROP TRIGGER IF EXISTS linux_effect_identity_unique;
DROP TRIGGER IF EXISTS simulated_effect_identity_unique;
DROP TRIGGER IF EXISTS linux_resolution_command_unique;
DROP TRIGGER IF EXISTS simulated_resolution_command_unique;

CREATE TRIGGER linux_effect_identity_unique
BEFORE INSERT ON linux_execution_effects
BEGIN
    SELECT RAISE(ABORT, 'operation identity already used by simulated execution')
    WHERE EXISTS (SELECT 1 FROM execution_effects WHERE operation_id = NEW.operation_id);
END;

CREATE TRIGGER simulated_effect_identity_unique
BEFORE INSERT ON execution_effects
BEGIN
    SELECT RAISE(ABORT, 'operation identity already used by linux execution')
    WHERE EXISTS (SELECT 1 FROM linux_execution_effects WHERE operation_id = NEW.operation_id);
END;

CREATE TRIGGER linux_resolution_command_unique
BEFORE INSERT ON linux_execution_resolutions
BEGIN
    SELECT RAISE(ABORT, 'resolution command identity already used by simulated execution')
    WHERE EXISTS (SELECT 1 FROM execution_resolutions WHERE command_id = NEW.command_id);
END;

CREATE TRIGGER simulated_resolution_command_unique
BEFORE INSERT ON execution_resolutions
BEGIN
    SELECT RAISE(ABORT, 'resolution command identity already used by linux execution')
    WHERE EXISTS (SELECT 1 FROM linux_execution_resolutions WHERE command_id = NEW.command_id);
END;
