-- Current plan text and its bound digest. Nullable on purpose.
-- Existing rows are not backfilled. missions.plan_digest may still hold a
-- pre-172b plan-text hash; approval does not read it.
ALTER TABLE attempts ADD COLUMN plan_body TEXT;
ALTER TABLE attempts ADD COLUMN plan_digest TEXT;
