-- 20260614000005_contracts.sql
-- Created: 2026-06-14 (task #7, doc/09-contracts.md + doc/99-integration-matrix.md §5)
--
-- Slave-contract durable schema. PostgreSQL is the source of
-- truth for contract state; the Rust gRPC service
-- (`biocapital-grpc::contract_service`) is the only writer.
--
-- Companion code:
--   - rust/crates/biocapital-contract/src/domain/contract.rs
--   - rust/crates/biocapital-contract/src/domain/lifecycle.rs
--   - rust/crates/biocapital-pg/src/contract.rs           (ContractRepository)
--   - rust/crates/biocapital-grpc/src/contract_service.rs (gRPC layer)
--
-- Idempotency: every CREATE uses IF NOT EXISTS so a partial
-- run can resume. 99 §2.2 audit invariants are preserved
-- (the `audit_*` family is written by BankService / DglabService
-- — `contracts` itself does not own an audit table; the
-- 4 contract events listed in 99 §3.1 are emitted via
-- `ContractResponse.event_meta` and logged by the Sable JNI
-- bridge through the existing `BankTransactionEvent` /
-- `audit_bank` channel for payout rows).

BEGIN;

-- ── 1. contracts (durable contract ledger — 09 §2.1 + §5.1) ────────────
--
-- One row per proposed contract. The five-state status enum is
-- enforced by the CHECK below; the lifecycle helpers in
-- `biocapital-contract::domain::lifecycle` are the only code
-- path that mutates this table.
--
-- Naming note: the user task spec uses `proposer_uuid` /
-- `acceptor_uuid`; this migration uses `master_uuid` /
-- `slave_uuid` to match the canonical doc 09 §2.1 + proto
-- `ContractProposeRequest` field names. The Rust domain type
-- `Contract` carries `proposer_uuid` / `acceptor_uuid`; the
-- gRPC layer maps at the request / response boundary.

CREATE TABLE IF NOT EXISTS contracts (
    contract_id         UUID         PRIMARY KEY,
    master_uuid         UUID         NOT NULL,
    slave_uuid          UUID         NOT NULL,
    status              VARCHAR(16)  NOT NULL
                        CHECK (status IN ('PROPOSED', 'ACTIVE', 'TERMINATED', 'REDEEMED', 'REJECTED')),
    terms_type          VARCHAR(32)  NOT NULL,
    terms_json          JSONB        NOT NULL,
    revenue_share_pct   FLOAT        NOT NULL DEFAULT 0.0
                        CHECK (revenue_share_pct >= 0 AND revenue_share_pct <= 100),
    redemption_cost     BIGINT       NOT NULL DEFAULT 0
                        CHECK (redemption_cost >= 0),
    expires_tick        BIGINT,
    created_tick        BIGINT       NOT NULL,
    updated_tick        BIGINT       NOT NULL,
    activated_tick      BIGINT,
    terminated_tick     BIGINT,
    redeemed_tick       BIGINT,
    reason              VARCHAR(256)
);

COMMENT ON TABLE  contracts                  IS 'Slave-contract ledger (09 §2.1 + §5.1)';
COMMENT ON COLUMN contracts.contract_id      IS 'UUIDv7 primary key';
COMMENT ON COLUMN contracts.master_uuid      IS 'Master (proposer) UUID — `biocapital-contract` domain field `proposer_uuid`';
COMMENT ON COLUMN contracts.slave_uuid       IS 'Slave (acceptor) UUID — domain field `acceptor_uuid`';
COMMENT ON COLUMN contracts.status           IS 'PROPOSED | ACTIVE | TERMINATED | REDEEMED | REJECTED';
COMMENT ON COLUMN contracts.terms_type       IS 'Free-form VARCHAR(32); KubeJS can inject new types (09 §2.2)';
COMMENT ON COLUMN contracts.terms_json       IS 'Open-ended JSONB parameters blob (09 §2.2)';
COMMENT ON COLUMN contracts.revenue_share_pct IS 'Master revenue share, 0..100';
COMMENT ON COLUMN contracts.redemption_cost  IS 'Cat-grass units the slave pays to redeem (09 §3.3)';
COMMENT ON COLUMN contracts.expires_tick     IS 'Auto-REJECT deadline; NULL = permanent';
COMMENT ON COLUMN contracts.created_tick     IS 'Server tick (ms) at INSERT';
COMMENT ON COLUMN contracts.updated_tick     IS 'Server tick (ms) of most recent mutation';
COMMENT ON COLUMN contracts.activated_tick   IS 'Server tick (ms) at PROPOSED→ACTIVE';
COMMENT ON COLUMN contracts.terminated_tick  IS 'Server tick (ms) at PROPOSED→REJECTED or ACTIVE→TERMINATED';
COMMENT ON COLUMN contracts.redeemed_tick    IS 'Server tick (ms) at ACTIVE→REDEEMED';
COMMENT ON COLUMN contracts.reason           IS 'Free-form termination / rejection reason (VARCHAR(256))';

-- "Both parties must be set" — prevent a self-contract slipping
-- through a buggy caller (the domain layer rejects this too;
-- CHECK is the last line of defence).
ALTER TABLE contracts
    DROP CONSTRAINT IF EXISTS contracts_distinct_parties;
ALTER TABLE contracts
    ADD CONSTRAINT contracts_distinct_parties
    CHECK (master_uuid <> slave_uuid);

CREATE INDEX IF NOT EXISTS idx_contracts_proposer
    ON contracts (master_uuid);
CREATE INDEX IF NOT EXISTS idx_contracts_acceptor
    ON contracts (slave_uuid);
CREATE INDEX IF NOT EXISTS idx_contracts_status
    ON contracts (status, updated_tick DESC);
-- Expiry sweeper scans for `expires_tick IS NOT NULL` rows;
-- partial index keeps the index small even when most
-- contracts are permanent.
CREATE INDEX IF NOT EXISTS idx_contracts_expires
    ON contracts (expires_tick)
    WHERE expires_tick IS NOT NULL;

-- ── 2. contract_payouts (auto-dividend / redemption log — 09 §5.1) ─────
--
-- One row per emitted payout. The `request_id` UNIQUE index
-- makes the payout idempotent under replayed RPCs (99 §2.2
-- + 09 §5.3); the bank-side `bank_transactions.request_id`
-- unique index is the upstream gate, this is the downstream
-- confirmation.
--
-- Foreign key to `contracts` cascades on delete so a manual
-- `DELETE FROM contracts …` cannot orphan payout history.

CREATE TABLE IF NOT EXISTS contract_payouts (
    payout_id        UUID         PRIMARY KEY,
    contract_id      UUID         NOT NULL
                      REFERENCES contracts(contract_id) ON DELETE CASCADE,
    from_account     UUID         NOT NULL,
    to_account       UUID         NOT NULL,
    amount           BIGINT       NOT NULL
                      CHECK (amount > 0),
    reason           VARCHAR(64)  NOT NULL,
    tick_millis      BIGINT       NOT NULL,
    request_id       UUID
);

COMMENT ON TABLE  contract_payouts                 IS 'Slave-contract payout ledger (09 §5.1)';
COMMENT ON COLUMN contract_payouts.payout_id       IS 'UUIDv7 primary key';
COMMENT ON COLUMN contract_payouts.contract_id     IS 'Owning contract; cascades on contracts delete';
COMMENT ON COLUMN contract_payouts.from_account    IS 'Debit account (slave / acceptor)';
COMMENT ON COLUMN contract_payouts.to_account      IS 'Credit account (master / proposer)';
COMMENT ON COLUMN contract_payouts.amount          IS 'Strictly positive cat-grass units (CHECK)';
COMMENT ON COLUMN contract_payouts.reason          IS 'DAILY_TRIBUTE | REDEMPTION | CUSTOM';
COMMENT ON COLUMN contract_payouts.tick_millis     IS 'Server tick (ms) when the bank transfer committed';
COMMENT ON COLUMN contract_payouts.request_id      IS 'Cross-table idempotency key (99 §2.2)';

-- Idempotency: replays of the same RPC hit the unique index
-- and the second insert fails. The gRPC layer turns the
-- failure into a return-the-prior-row response.
CREATE UNIQUE INDEX IF NOT EXISTS idx_contract_payouts_request_id
    ON contract_payouts (request_id);

CREATE INDEX IF NOT EXISTS idx_contract_payouts_contract_time
    ON contract_payouts (contract_id, tick_millis DESC);

COMMIT;