-- Attempt inputs frozen at draft -> planning, plus the owner generation
-- that held the exclusive host lock when planning started.
ALTER TABLE attempts ADD COLUMN frozen_inputs_json TEXT;
ALTER TABLE attempts ADD COLUMN owner_generation INTEGER;
