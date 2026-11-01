-- GO must never precede durable release evidence. Existing v7 unresolved
-- workloads have no such evidence, so conservatively treat them as released.
-- This is an uncertainty marker for old rows, not a reconstructed GO time.
ALTER TABLE linux_execution_effects ADD COLUMN released_at INTEGER;
UPDATE linux_execution_effects SET released_at = created_at
WHERE cgroup_path IS NOT NULL AND state IN ('prepared','entered','running','uncertain');

-- Earlier development versions of v7 lacked the entered CHECK. Enforce it
-- for future writes without rebuilding the table or rewriting old receipts.
CREATE TRIGGER linux_entered_evidence_insert
BEFORE INSERT ON linux_execution_effects
WHEN NEW.state = 'entered' AND NEW.entered_at IS NULL
BEGIN
    SELECT RAISE(ABORT, 'entered linux effect requires entry evidence');
END;
CREATE TRIGGER linux_entered_evidence_update
BEFORE UPDATE ON linux_execution_effects
WHEN NEW.state = 'entered' AND NEW.entered_at IS NULL
BEGIN
    SELECT RAISE(ABORT, 'entered linux effect requires entry evidence');
END;
