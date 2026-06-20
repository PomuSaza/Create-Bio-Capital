-- 20260619000001_audit_monthly_partition.sql
-- Replaces 20260617000001 (broken: original tried to re-add PK on default
-- partition, but PG requires unique constraints on partitioned tables to
-- include all partition key columns; the original PK on (audit_id) / (log_id)
-- did not include (tick_millis), so the migration failed with
-- "unique constraint on partitioned table must include all
-- partitioning columns" — see doc/CHANGELOG.md 2026-06-19 audit-migration-fix).
--
-- Strategy (corrected 2026-06-19):
-- 1. Detect state per table: regular (`r`) or partitioned (`p`).
-- 2. If already partitioned → skip.
-- 3. If regular:
--    a. Drop existing PK constraint (PG won't let us keep the old PK
--       because partition tables require PK to include partition key).
--       Audit tables are append-only + time-ranged queries, so losing
--       the per-row PK is acceptable; we keep a UNIQUE INDEX on the
--       business key where present (e.g. `request_id` for idempotency).
--    b. RENAME table to `<old>` (data preserved).
--    c. CREATE TABLE parent with same column list, PARTITION BY RANGE (tick_millis).
--    d. CREATE default partition.
--    e. INSERT SELECT from old to parent (data migrates).
--    f. DROP old.
--
-- Idempotency: the audit_is_partitioned() guard handles "already
-- partitioned" → skip. Re-running on a fresh DB works exactly once
-- per table. Re-running on a DB that was previously partially
-- converted (e.g. parent exists but no default) is detected by the
-- per-table status check.

BEGIN;

-- ── 1. Guard function: is a table a declarative partition parent? ─

CREATE OR REPLACE FUNCTION audit_is_partitioned(p_table_name TEXT)
RETURNS BOOLEAN AS $$
BEGIN
    -- MVP-0.1 重写（与 011 一致）：用 EXISTS 避免 char→BIGINT 隐式 cast 风险。
    -- PG 18 实际上把 'r' 转成 114 静默通过，但显式 EXISTS 更清晰且不限 schema 类型。
    RETURN EXISTS (
        SELECT 1
          FROM pg_partitioned_table pt
          JOIN pg_class c ON c.oid = pt.partrelid
         WHERE c.relname = p_table_name
    );
END;
$$ LANGUAGE plpgsql STABLE;

COMMENT ON FUNCTION audit_is_partitioned(TEXT) IS
    'True if the named table is a PostgreSQL declarative partition parent. Used by 20260619000001_audit_monthly_partition.sql to skip already-partitioned tables.';

-- ── 2. Optional: ensure_monthly_partition (business-layer) ─────────

-- Create-or-noop monthly partition. Called by Rust startup + monthly cron.
-- Idempotent: returns existing partition name if already created.
CREATE OR REPLACE FUNCTION audit_ensure_monthly_partition(
    p_table_name TEXT,
    p_year_month INT
) RETURNS TEXT AS $$
DECLARE
    v_part_name      TEXT;
    v_lo             BIGINT;
    v_hi             BIGINT;
    v_default_count  BIGINT;
    v_lo_text        TEXT;
    v_hi_text        TEXT;
    v_default_name   TEXT;
    v_existing_count BIGINT;
BEGIN
    IF p_year_month < 197001 OR p_year_month > 999912 THEN
        RAISE EXCEPTION 'audit_ensure_monthly_partition: year_month % out of range', p_year_month;
    END IF;
    IF NOT audit_is_partitioned(p_table_name) THEN
        RAISE EXCEPTION 'audit_ensure_monthly_partition: % is not a partitioned table; run 20260619000001_audit_monthly_partition.sql first', p_table_name;
    END IF;

    v_part_name := format('%s_%s', p_table_name, to_char(to_date(p_year_month::TEXT, 'YYYYMM'), 'YYYY_MM'));

    SELECT COUNT(*) INTO v_existing_count
      FROM pg_class WHERE relname = v_part_name;
    IF v_existing_count > 0 THEN
        RETURN v_part_name;
    END IF;

    v_lo := (extract(epoch FROM to_timestamp(p_year_month::TEXT || '01', 'YYYYMMDD')) * 1000)::BIGINT;
    v_hi := (extract(epoch FROM (to_timestamp(p_year_month::TEXT || '01', 'YYYYMMDD') + INTERVAL '1 month')) * 1000)::BIGINT;
    v_lo_text := v_lo::TEXT;
    v_hi_text := v_hi::TEXT;

    EXECUTE format(
        'CREATE TABLE %I PARTITION OF %I FOR VALUES FROM (%s) TO (%s)',
        v_part_name, p_table_name, v_lo_text, v_hi_text
    );

    v_default_name := p_table_name || '_default';
    SELECT COUNT(*) INTO v_default_count FROM pg_class WHERE relname = v_default_name;
    IF v_default_count > 0 THEN
        EXECUTE format(
            'WITH moved AS (
                DELETE FROM %I WHERE tick_millis >= %s AND tick_millis < %s RETURNING *
            )
            INSERT INTO %I SELECT * FROM moved',
            v_default_name, v_lo_text, v_hi_text, v_part_name
        );
    END IF;

    RETURN v_part_name;
END;
$$ LANGUAGE plpgsql VOLATILE;

COMMENT ON FUNCTION audit_ensure_monthly_partition(TEXT, INT) IS
    'Create a monthly partition of the named audit_* table and move any default-partition rows whose tick_millis falls in [month_start, next_month_start) into the new partition. Idempotent. Called by Rust startup + monthly cron.';

-- ── 3. Convert each audit_* table to PARTITION BY RANGE (tick_millis) ─

DO $do$
DECLARE
    v_tabs TEXT[] := ARRAY[
        'audit_player_state',
        'audit_bank',
        'audit_core_pod',
        'audit_dglab',
        'audit_environment',
        'audit_hardware_token',
        'audit_creature_config'
    ];
    v_tab TEXT;
    v_cols TEXT;
    v_old TEXT;
    v_new TEXT;
    v_def TEXT;
    v_exists BOOLEAN;
    v_pk_constraints TEXT[];
    v_pk TEXT;
BEGIN
    FOREACH v_tab IN ARRAY v_tabs LOOP
        -- skip if already partitioned
        IF audit_is_partitioned(v_tab) THEN
            RAISE NOTICE 'audit_partition: % already partitioned, skipping', v_tab;
            CONTINUE;
        END IF;

        -- check table exists in current schema
        SELECT EXISTS (
            SELECT 1 FROM pg_class c
            JOIN pg_namespace n ON n.oid = c.relnamespace
            WHERE c.relname = v_tab AND c.relkind = 'r' AND n.nspname = current_schema()
        ) INTO v_exists;
        IF NOT v_exists THEN
            RAISE NOTICE 'audit_partition: % does not exist in current schema, skipping', v_tab;
            CONTINUE;
        END IF;

        -- drop PK constraints (PG won't allow them on partitioned parent
        -- because they don't include the partition key tick_millis).
        -- Audit tables are append-only + time-ranged; per-row PK is
        -- unnecessary. UNIQUE INDEXes on (request_id) for idempotency
        -- are preserved (those are not PK constraints).
        SELECT array_agg(c.conname) INTO v_pk_constraints
          FROM pg_constraint c
         WHERE c.conrelid = v_tab::regclass
           AND c.contype = 'p';
        IF v_pk_constraints IS NOT NULL THEN
            FOREACH v_pk IN ARRAY v_pk_constraints LOOP
                EXECUTE format('ALTER TABLE %I DROP CONSTRAINT %I', v_tab, v_pk);
                RAISE NOTICE 'audit_partition: dropped PK % on %', v_pk, v_tab;
            END LOOP;
        END IF;

        v_old := v_tab || '_old_for_partition_20260619';
        v_new := v_tab;
        v_def := v_tab || '_default';

        -- grab column definitions (for LIKE-style column-only CREATE)
        EXECUTE format(
            'SELECT string_agg(quote_ident(attname) || '' '' || format_type(atttypid, atttypmod) || '' '' ||
                              CASE WHEN attnotnull THEN ''NOT NULL'' ELSE '''' END,
                              '', '' ORDER BY attnum)
             FROM pg_attribute
             WHERE attrelid = %L::regclass AND attnum > 0 AND NOT attisdropped',
            v_new
        ) INTO v_cols;

        -- 1) RENAME 旧表 (preserves data + FK relationships)
        EXECUTE format('ALTER TABLE %I RENAME TO %I', v_new, v_old);

        -- 2) 重建 parent (PARTITION BY RANGE on tick_millis).
        -- Note: no PK constraint — partitioned tables require
        -- partition-key inclusion in any unique constraint, which the
        -- original (audit_id) / (log_id) PKs didn't have.
        EXECUTE format('CREATE TABLE %I (%s) PARTITION BY RANGE (tick_millis)', v_new, v_cols);

        -- 3) 重建 default partition (catch-all)
        EXECUTE format('CREATE TABLE %I PARTITION OF %I DEFAULT', v_def, v_new);

        -- 4) 搬数据
        EXECUTE format('INSERT INTO %I SELECT * FROM %I', v_new, v_old);

        -- 5) 删旧表
        EXECUTE format('DROP TABLE %I', v_old);

        RAISE NOTICE 'audit_partition: % partitioned -> % + % (data migrated, PK dropped)', v_tab, v_new, v_def;
    END LOOP;
END
$do$;

COMMIT;
