-- Additive supersession audit. v1–v8 effect, receipt, and resolution rows are
-- not rewritten. Effect-state CHECKs stay as they are: blocking queries exclude
-- superseded operations instead of introducing a new state. A supersession
-- cannot be updated, deleted, or superseded, and its command id is unique
-- across both resolution journals.

DROP TRIGGER IF EXISTS linux_resolution_command_unique;
DROP TRIGGER IF EXISTS simulated_resolution_command_unique;
DROP TRIGGER IF EXISTS supersession_command_unique;
DROP TRIGGER IF EXISTS execution_supersessions_no_update;
DROP TRIGGER IF EXISTS execution_supersessions_no_delete;
DROP TRIGGER IF EXISTS execution_resolutions_no_update;
DROP TRIGGER IF EXISTS execution_resolutions_no_delete;
DROP TRIGGER IF EXISTS linux_resolutions_no_update;
DROP TRIGGER IF EXISTS linux_resolutions_no_delete;

CREATE TABLE IF NOT EXISTS execution_supersessions (
    command_id TEXT PRIMARY KEY,
    operation_id TEXT NOT NULL UNIQUE,
    journal TEXT NOT NULL CHECK (journal IN ('simulated', 'linux')),
    abandonment_command_id TEXT NOT NULL UNIQUE,
    request_hash TEXT NOT NULL,
    audit_json TEXT NOT NULL
);

CREATE TRIGGER execution_supersessions_no_update
BEFORE UPDATE ON execution_supersessions
BEGIN
    SELECT RAISE(ABORT, 'execution supersessions are append-only');
END;

CREATE TRIGGER execution_supersessions_no_delete
BEFORE DELETE ON execution_supersessions
BEGIN
    SELECT RAISE(ABORT, 'execution supersessions are append-only');
END;

CREATE TRIGGER execution_resolutions_no_update
BEFORE UPDATE ON execution_resolutions
BEGIN
    SELECT RAISE(ABORT, 'execution resolutions are append-only');
END;

CREATE TRIGGER execution_resolutions_no_delete
BEFORE DELETE ON execution_resolutions
BEGIN
    SELECT RAISE(ABORT, 'execution resolutions are append-only');
END;

CREATE TRIGGER linux_resolutions_no_update
BEFORE UPDATE ON linux_execution_resolutions
BEGIN
    SELECT RAISE(ABORT, 'execution resolutions are append-only');
END;

CREATE TRIGGER linux_resolutions_no_delete
BEFORE DELETE ON linux_execution_resolutions
BEGIN
    SELECT RAISE(ABORT, 'execution resolutions are append-only');
END;

CREATE TRIGGER linux_resolution_command_unique
BEFORE INSERT ON linux_execution_resolutions
BEGIN
    SELECT RAISE(ABORT, 'resolution command identity already used')
    WHERE EXISTS (SELECT 1 FROM execution_resolutions WHERE command_id = NEW.command_id)
       OR EXISTS (SELECT 1 FROM execution_supersessions WHERE command_id = NEW.command_id);
END;

CREATE TRIGGER simulated_resolution_command_unique
BEFORE INSERT ON execution_resolutions
BEGIN
    SELECT RAISE(ABORT, 'resolution command identity already used')
    WHERE EXISTS (SELECT 1 FROM linux_execution_resolutions WHERE command_id = NEW.command_id)
       OR EXISTS (SELECT 1 FROM execution_supersessions WHERE command_id = NEW.command_id);
END;

CREATE TRIGGER supersession_command_unique
BEFORE INSERT ON execution_supersessions
BEGIN
    SELECT RAISE(ABORT, 'supersession command identity already used')
    WHERE EXISTS (SELECT 1 FROM execution_resolutions WHERE command_id = NEW.command_id)
       OR EXISTS (SELECT 1 FROM linux_execution_resolutions WHERE command_id = NEW.command_id);
END;
