-- Tonies sync run (#502): make the concurrency guard race-free.
--
-- `claim_run` used to check for a live `running` row and then INSERT in three
-- separate statements — a TOCTOU window in which two concurrent
-- `POST /tonies/{id}/sync` requests could both insert a `running` row and start
-- two parallel runs for the same binding.
--
-- This partial UNIQUE index closes that window at the storage layer: at most
-- one `running` row may exist per binding, so a racing INSERT fails with a
-- UNIQUE-constraint error (SQLite 2067), which `claim_run` maps to
-- `Claim::Busy`. Finished rows (`ok`/`noop`/`timeout`/`error`) are not
-- constrained, so the run log stays unbounded.
--
-- Additive only (CREATE INDEX), number free (058 → 059).

CREATE UNIQUE INDEX IF NOT EXISTS idx_hub_tonie_syncs_running
    ON hub_tonie_syncs(binding_id) WHERE status = 'running';

SELECT 'Migration 059 applied: one running tonie sync per binding' AS status;
