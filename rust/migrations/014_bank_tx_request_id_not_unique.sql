-- 20260619000002_bank_tx_request_id_not_unique.sql
-- Created: 2026-06-19 (E2E test #46 — fix transfer two-side insert).
--
-- Problem: migration 20260614000002_bank.sql created
--   CREATE UNIQUE INDEX idx_bank_tx_request_id ON bank_transactions (request_id);
-- which is correct for DEPOSIT / WITHDRAW (one row per request), but
-- BREAKS atomic_transfer() because the Rust repo inserts TWO rows
-- with the SAME request_id (TRANSFER_OUT for `from` + TRANSFER_IN for
-- `to`) — the second insert fails with `duplicate key value violates
-- unique constraint "idx_bank_tx_request_id"`.
--
-- Fix: drop the unique index, replace with a non-unique btree index
-- for query speed. The idempotency guarantee for `atomic_*` is now
-- enforced by `fetch_by_request_id(...)` in the Rust repo (08 §5.3);
-- the database no longer prevents duplicate request_id values.
--
-- Idempotency: `DROP INDEX IF EXISTS` + `CREATE INDEX IF NOT EXISTS`
-- so this migration is safe to re-run.

BEGIN;

DROP INDEX IF EXISTS idx_bank_tx_request_id;

CREATE INDEX IF NOT EXISTS idx_bank_tx_request_id_non_unique
    ON bank_transactions (request_id);

COMMENT ON INDEX idx_bank_tx_request_id_non_unique IS
    'Non-unique btree on bank_transactions.request_id (08 §5.3 idempotency anchor — uniqueness enforced in app via fetch_by_request_id, NOT in DB because atomic_transfer inserts two rows with the same request_id for TRANSFER_OUT + TRANSFER_IN). Replaces the unique idx_bank_tx_request_id from 20260614000002_bank.sql after E2E test #46 surfaced the constraint conflict.';

COMMIT;