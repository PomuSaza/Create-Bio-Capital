-- 20260614000002_bank.sql
-- Created: 2026-06-14 (task #4, doc/08-bank.md + doc/99-integration-matrix.md §5)
--
-- Bank ledger durable schema. PostgreSQL is the source of truth for the
-- Rust gRPC service; the Java side holds a NeoForge `BankManager`
-- cache that mirrors these rows. The Java side is the
-- grey-decommission baseline (see doc/08-bank.md §1.1); new writes
-- go through `BankService` over gRPC.
--
-- Companion code:
--   - rust/crates/biocapital-bank/src/domain/{account,transfer,batch}.rs (domain types)
--   - rust/crates/biocapital-pg/src/bank.rs        (BankRepository)
--   - rust/crates/biocapital-grpc/src/bank_service.rs (gRPC layer)
--
-- Idempotency: every CREATE uses IF NOT EXISTS so a partial run can resume.

BEGIN;

-- ── 1. bank_accounts (per-player ledger) ─────────────────────────────────
--
-- One row per player. `account_uuid` is the durable primary key;
-- `owner_uuid` is a unique index used for "look up by player" (e.g.
-- when a card is first signed — see 08 §3.5). The `balance` CHECK
-- mirrors the `MAX_BALANCE` constant in `biocapital_bank::domain`
-- (100,000,000); changing the constant requires a coordinated
-- migration per 99 §10.2.

CREATE TABLE IF NOT EXISTS bank_accounts (
    account_uuid  UUID        PRIMARY KEY,
    owner_uuid    UUID        NOT NULL,
    balance       BIGINT      NOT NULL DEFAULT 0
                  CHECK (balance >= 0 AND balance <= 100000000),
    max_balance   BIGINT      NOT NULL DEFAULT 100000000
                  CHECK (max_balance >= 1),
    device_lock   VARCHAR(256),
    created_tick  BIGINT      NOT NULL,
    updated_tick  BIGINT      NOT NULL
);

COMMENT ON TABLE  bank_accounts              IS 'Per-player bank account (08 §1 / §5.1)';
COMMENT ON COLUMN bank_accounts.account_uuid IS 'Durable primary key';
COMMENT ON COLUMN bank_accounts.owner_uuid   IS 'Player that owns the account; one account per player';
COMMENT ON COLUMN bank_accounts.balance      IS 'Cat-grass units; CHECK mirrors MAX_BALANCE=100,000,000';
COMMENT ON COLUMN bank_accounts.max_balance  IS 'Per-account cap; default 100,000,000 (08 §1.2)';
COMMENT ON COLUMN bank_accounts.device_lock  IS 'Optional device fingerprint; null = unlocked (08 §3.5)';
COMMENT ON COLUMN bank_accounts.created_tick IS 'Server tick (ms) of first insert';
COMMENT ON COLUMN bank_accounts.updated_tick IS 'Server tick (ms) of most recent mutation';

CREATE UNIQUE INDEX IF NOT EXISTS idx_bank_accounts_owner
    ON bank_accounts (owner_uuid);

CREATE INDEX IF NOT EXISTS idx_bank_accounts_updated
    ON bank_accounts (updated_tick DESC);

-- ── 2. cat_grass_batches (batch tracking — 08 §2.2) ──────────────────────
--
-- Every blade of cat-grass traces back to one of these rows. The
-- `batch_id` is NOT stored on the in-world `ItemStack`; physical
-- cat-grass is anonymous (08 §2.1). The cat-grass item has no
-- DataComponent, so the lookup from an item stack to a batch is
-- only possible via the holder's account / inventory snapshot at
-- audit time.

CREATE TABLE IF NOT EXISTS cat_grass_batches (
    batch_id            UUID        PRIMARY KEY,
    producer_uuid       UUID,
    production_tick     BIGINT      NOT NULL,
    production_source   VARCHAR(32) NOT NULL
                        CHECK (production_source IN ('ATM_DEPOSIT', 'BIOCAPITAL_REWARD', 'ADMIN_ISSUE')),
    total_amount        BIGINT      NOT NULL
                        CHECK (total_amount > 0),
    remaining_amount    BIGINT      NOT NULL
                        CHECK (remaining_amount >= 0),
    current_holder_uuid UUID
);

COMMENT ON TABLE  cat_grass_batches                  IS 'Cat-grass batch provenance (08 §2.2)';
COMMENT ON COLUMN cat_grass_batches.batch_id         IS 'UUIDv7 batch identifier';
COMMENT ON COLUMN cat_grass_batches.producer_uuid    IS 'Producer (player or system); nullable for unattributed origins';
COMMENT ON COLUMN cat_grass_batches.production_source IS 'ATM_DEPOSIT | BIOCAPITAL_REWARD | ADMIN_ISSUE';
COMMENT ON COLUMN cat_grass_batches.total_amount     IS 'Immutable amount produced';
COMMENT ON COLUMN cat_grass_batches.remaining_amount IS 'Amount still in circulation; consumes down to 0';
COMMENT ON COLUMN cat_grass_batches.current_holder_uuid IS 'Account holding the batch; null = physical inventory';

-- Partial index: most batches have a non-null holder; skipping the
-- null rows keeps the index small.
CREATE INDEX IF NOT EXISTS idx_cat_grass_batches_holder
    ON cat_grass_batches (current_holder_uuid)
    WHERE current_holder_uuid IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_cat_grass_batches_producer
    ON cat_grass_batches (producer_uuid)
    WHERE producer_uuid IS NOT NULL;

-- ── 3. bank_transactions (per-account ledger history) ────────────────────
--
-- Append-only history. The (request_id) unique index is the
-- idempotency anchor: every mutating RPC carries a request_id, and
-- the repository short-circuits if the id is already present
-- (08 §5.3). Pagination uses the (account_uuid, tick_millis DESC)
-- composite — see HistoryRequest.before_tick_millis in proto.

CREATE TABLE IF NOT EXISTS bank_transactions (
    tx_id              UUID        PRIMARY KEY,
    account_uuid       UUID        NOT NULL
                       REFERENCES bank_accounts(account_uuid) ON DELETE CASCADE,
    op                 VARCHAR(16) NOT NULL
                       CHECK (op IN ('DEPOSIT', 'WITHDRAW', 'TRANSFER_OUT', 'TRANSFER_IN')),
    amount             BIGINT      NOT NULL
                       CHECK (amount > 0),
    balance_after      BIGINT      NOT NULL
                       CHECK (balance_after >= 0),
    counterparty_uuid  UUID,
    counterparty_name  VARCHAR(64),
    tick_millis        BIGINT      NOT NULL,
    request_id         UUID
);

COMMENT ON TABLE  bank_transactions                  IS 'Per-account transaction history (08 §5.1)';
COMMENT ON COLUMN bank_transactions.tx_id            IS 'UUIDv7 primary key';
COMMENT ON COLUMN bank_transactions.account_uuid     IS 'Owner account; cascades on account delete';
COMMENT ON COLUMN bank_transactions.op               IS 'DEPOSIT | WITHDRAW | TRANSFER_OUT | TRANSFER_IN';
COMMENT ON COLUMN bank_transactions.amount           IS 'Strictly positive (direction in op)';
COMMENT ON COLUMN bank_transactions.balance_after    IS 'Snapshot of account balance post-commit';
COMMENT ON COLUMN bank_transactions.counterparty_uuid  IS 'Counter-account on transfer; null for plain dep/wd';
COMMENT ON COLUMN bank_transactions.counterparty_name  IS 'Display name (player name) of counterparty';
COMMENT ON COLUMN bank_transactions.tick_millis      IS 'Server tick (ms) at commit';
COMMENT ON COLUMN bank_transactions.request_id       IS 'Idempotency key; unique (08 §5.3)';

-- Idempotency: a given request_id maps to at most one transaction.
-- Uniqueness is the dedupe anchor for the gRPC layer.
CREATE UNIQUE INDEX IF NOT EXISTS idx_bank_tx_request_id
    ON bank_transactions (request_id);

-- History pagination (08 §3.4 面板 3 / 14 §3.2 HistoryRequest).
CREATE INDEX IF NOT EXISTS idx_bank_tx_account_time
    ON bank_transactions (account_uuid, tick_millis DESC);

-- ── 4. audit_bank (append-only audit log — 99 §2.2) ──────────────────────
--
-- Mirrors the `audit_player_state` shape from migration
-- 20260614000001. We do not denormalise the balance diff into a
-- JSONB `before/after` (bank rows have a single `balance` integer,
-- not a struct), so the columns are flat BIGINTs.

CREATE TABLE IF NOT EXISTS audit_bank (
    log_id              UUID        PRIMARY KEY,
    actor_uuid          UUID        NOT NULL,
    actor_type          VARCHAR(16) NOT NULL
                        CHECK (actor_type IN ('PLAYER', 'ADMIN_CMD', 'RUST_SERVICE', 'HARDWARE_DGLAB')),
    target_account_uuid UUID        NOT NULL,
    op                  VARCHAR(16) NOT NULL,
    before_balance      BIGINT      NOT NULL,
    after_balance       BIGINT      NOT NULL,
    tick_millis         BIGINT      NOT NULL,
    request_id          UUID,
    notes               JSONB
);

COMMENT ON TABLE  audit_bank                  IS 'Append-only audit for BankService (99 §2.2)';
COMMENT ON COLUMN audit_bank.log_id           IS 'UUIDv7 primary key';
COMMENT ON COLUMN audit_bank.actor_uuid       IS 'Player or service UUID that initiated the action';
COMMENT ON COLUMN audit_bank.actor_type       IS 'PLAYER | ADMIN_CMD | RUST_SERVICE | HARDWARE_DGLAB';
COMMENT ON COLUMN audit_bank.target_account_uuid IS 'bank_accounts.account_uuid affected';
COMMENT ON COLUMN audit_bank.op               IS 'bank.deposit | bank.withdraw | bank.transfer | bank.lock_device | bank.unlock_device | bank.invite';
COMMENT ON COLUMN audit_bank.before_balance   IS 'Account balance pre-mutation';
COMMENT ON COLUMN audit_bank.after_balance    IS 'Account balance post-mutation';
COMMENT ON COLUMN audit_bank.tick_millis      IS 'Server tick (ms) at audit emit';
COMMENT ON COLUMN audit_bank.request_id       IS 'Cross-table idempotency key (matches bank_transactions.request_id)';
COMMENT ON COLUMN audit_bank.notes            IS 'Optional context (counterparty, device_id, etc.)';

CREATE INDEX IF NOT EXISTS idx_audit_bank_target_time
    ON audit_bank (target_account_uuid, tick_millis DESC);

CREATE INDEX IF NOT EXISTS idx_audit_bank_op_time
    ON audit_bank (op, tick_millis DESC);

COMMIT;
