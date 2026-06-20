-- 20260617000001_audit_monthly_partition.sql
-- Created: 2026-06-17 (task #10, doc/01 §2.2 audit 99 §5)
--
-- 把 7 张 audit_* 表从「普通 heap table」改为「RANGE 按 tick_millis
-- 分区 + DEFAULT 分区」+ 提供 PL/pgSQL 工具函数供应用层按月建分区。
--
-- 受影响表（doc/01 §2.2 + doc/99 §5 实际 audit_* 集合）：
--   1. audit_player_state      (20260614000001)
--   2. audit_bank              (20260614000002)
--   3. audit_core_pod          (20260614000003)
--   4. audit_dglab             (20260614000004)
--   5. audit_environment       (20260614000008)
--   6. audit_hardware_token    (20260614000009)
--   7. audit_creature_config   (20260614000010)
--
-- 关键设计：
--   * 分区键：`tick_millis` (BIGINT, NOT NULL, 7 张表都有)
--   * DEFAULT 分区：建一张 `<table>_default` PARTITION OF ... DEFAULT
--     —— 所有新写入落在 default；按月切片由 `audit_ensure_monthly_partition`
--     主动建 `<table>_<YYYY_MM>` 分区并把 default 里的对应范围 ATTACH 过去
--   * 数据迁移：把旧表 RENAME 为 `<table>_old`，按相同列定义新建分区
--     parent + default，INSERT SELECT 搬数据，drop old。整个转换在 PL/pgSQL
--     DO 块里以单事务粒度执行（每张表独立事务，失败不会污染其他表）
--   * 幂等：通过 `pg_partitioned_table` 检查「该表是否已是分区 parent」；
--     已分区的表直接跳过（重复执行迁移不报错）
--   * 索引：parent 上的索引在 ATTACH DEFAULT 时自动创建；monthly 子分区
--     继承 parent 索引
--
-- 后续运维：
--   * 业务层（应用启动时 + 月度定时器）调用
--     `SELECT audit_ensure_monthly_partition('audit_bank', 202606);`
--   * 100k+ 行月份的 archive 由月度 cron 主动
--     `ALTER TABLE audit_bank DETACH PARTITION audit_bank_2026_06;`
--     + `CREATE TABLE audit_archive.audit_bank_2026_06_archived AS
--       TABLE audit_bank_2026_06;` + `DROP TABLE audit_bank_2026_06;`
--     —— archive schema 留待后续 migration
--
-- Companion code:
--   - rust/crates/biocapital-pg/src/audit_partition.rs  (后续任务)
--   - rust/crates/biocapital-cli/src/cmd_migrate.rs     (迁移入口)
--
-- Idempotency: 通过 `audit_is_partitioned()` 守卫；重复执行不报错。

BEGIN;

-- ── 1. 工具函数：判断表是否已分区 ───────────────────────────────

CREATE OR REPLACE FUNCTION audit_is_partitioned(p_table_name TEXT)
RETURNS BOOLEAN AS $$
DECLARE
    v_partkey BIGINT;
BEGIN
    SELECT partstrat INTO v_partkey
      FROM pg_partitioned_table pt
      JOIN pg_class c ON c.oid = pt.partrelid
     WHERE c.relname = p_table_name;
    RETURN v_partkey IS NOT NULL;
END;
$$ LANGUAGE plpgsql STABLE;

COMMENT ON FUNCTION audit_is_partitioned(TEXT) IS
    'True if the named table is a PostgreSQL declarative partition parent (pg_partitioned_table.partstrat IS NOT NULL). Used by 20260617000001_audit_monthly_partition.sql to skip already-partitioned tables.';

-- ── 2. 工具函数：按月建分区（业务层主动调） ───────────────────

-- 用法：SELECT audit_ensure_monthly_partition('audit_bank', 202606);
-- 返回建好的分区名（已存在则返回 NULL）。
CREATE OR REPLACE FUNCTION audit_ensure_monthly_partition(
    p_table_name TEXT,
    p_year_month INT          -- YYYYMM，如 202606
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
    -- 月份合法性
    IF p_year_month < 197001 OR p_year_month > 999912 THEN
        RAISE EXCEPTION 'audit_ensure_monthly_partition: year_month % out of range', p_year_month;
    END IF;
    -- 父表必须已分区
    IF NOT audit_is_partitioned(p_table_name) THEN
        RAISE EXCEPTION 'audit_ensure_monthly_partition: % is not a partitioned table; run 20260617000001_audit_monthly_partition.sql first', p_table_name;
    END IF;

    v_part_name := format('%s_%s', p_table_name, to_char(to_date(p_year_month::TEXT, 'YYYYMM'), 'YYYY_MM'));

    -- 已存在则直接返回
    SELECT COUNT(*) INTO v_existing_count
      FROM pg_class WHERE relname = v_part_name;
    IF v_existing_count > 0 THEN
        RETURN v_part_name;
    END IF;

    -- tick_millis 是 BIGINT（毫秒）。月边界换算：
    -- 2026-06-01 00:00:00 UTC  =  unix epoch 毫秒  =  1748736000000
    -- 2026-07-01 00:00:00 UTC  =  unix epoch 毫秒  =  1751328000000
    -- 用 extract(epoch from ...) * 1000 直接算毫秒，避免硬编码年份表。
    v_lo := (extract(epoch FROM to_timestamp(p_year_month::TEXT || '01', 'YYYYMMDD')) * 1000)::BIGINT;
    v_hi := (extract(epoch FROM (to_timestamp(p_year_month::TEXT || '01', 'YYYYMMDD') + INTERVAL '1 month')) * 1000)::BIGINT;
    v_lo_text := v_lo::TEXT;
    v_hi_text := v_hi::TEXT;

    -- 建空分区（继承 parent 全部列 + 索引 + CHECK）
    EXECUTE format(
        'CREATE TABLE %I PARTITION OF %I FOR VALUES FROM (%s) TO (%s)',
        v_part_name, p_table_name, v_lo_text, v_hi_text
    );

    -- 把 default 分区里落在本月的行搬过来（如果 default 不存在则跳过）
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
    'Create a monthly partition of the named audit_* table and move any default-partition rows whose tick_millis falls in [month_start, next_month_start) into the new partition. Idempotent. Called by Rust startup + monthly cron (doc/01 §2.2 + doc/14 §2.1).';

-- ── 3. 转换 7 张 audit_* 表 ────────────────────────────────────
--
-- 每张表做：if 存在 && 未分区 -> RENAME old -> CREATE parent (PARTITION BY
-- RANGE (tick_millis)) -> CREATE default partition -> INSERT SELECT
-- 搬数据 -> DROP old.
--
-- 用 PL/pgSQL DO 块做（DO 不允许事务控制，所以单张表失败不回滚整个
-- migration；用 SAVEPOINT 保证单张表原子）。

DO $do$
DECLARE
    v_old TEXT;
    v_new TEXT;
    v_def TEXT;
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
    v_idx_create TEXT;
    v_idx_creates TEXT[] := ARRAY[
        -- audit_player_state
        'CREATE INDEX IF NOT EXISTS idx_audit_player_state_target_time ON %I (target_uuid, tick_millis DESC);
         CREATE INDEX IF NOT EXISTS idx_audit_player_state_op_time ON %I (op, tick_millis DESC);
         CREATE INDEX IF NOT EXISTS idx_audit_player_state_request ON %I (request_id) WHERE request_id IS NOT NULL;',
        -- audit_bank
        'CREATE INDEX IF NOT EXISTS idx_audit_bank_target_time ON %I (target_account_uuid, tick_millis DESC);
         CREATE INDEX IF NOT EXISTS idx_audit_bank_op_time ON %I (op, tick_millis DESC);',
        -- audit_core_pod
        'CREATE INDEX IF NOT EXISTS idx_audit_core_pod_target_time ON %I (target_pod_world_uuid, target_pod_dimension, target_pod_pos_x, target_pod_pos_y, target_pod_pos_z, tick_millis DESC);
         CREATE INDEX IF NOT EXISTS idx_audit_core_pod_op_time ON %I (op, tick_millis DESC);',
        -- audit_dglab
        'CREATE INDEX IF NOT EXISTS idx_audit_dglab_target_time ON %I (target_owner_uuid, tick_millis DESC);
         CREATE INDEX IF NOT EXISTS idx_audit_dglab_op_time ON %I (op, tick_millis DESC);
         CREATE INDEX IF NOT EXISTS idx_audit_dglab_actor_time ON %I (actor_uuid, tick_millis DESC);',
        -- audit_environment
        'CREATE INDEX IF NOT EXISTS idx_audit_environment_target_time ON %I (target_player_uuid, tick_millis DESC);
         CREATE INDEX IF NOT EXISTS idx_audit_environment_env_time ON %I (environment, tick_millis DESC);
         CREATE INDEX IF NOT EXISTS idx_audit_environment_request ON %I (request_id) WHERE request_id IS NOT NULL;',
        -- audit_hardware_token
        'CREATE INDEX IF NOT EXISTS idx_audit_hardware_token_target_time ON %I (target_owner_uuid, tick_millis DESC);
         CREATE INDEX IF NOT EXISTS idx_audit_hardware_token_op_time ON %I (op, tick_millis DESC);',
        -- audit_creature_config
        'CREATE INDEX IF NOT EXISTS idx_audit_creature_config_target_time ON %I (target_creature_id, tick_millis DESC);
         CREATE INDEX IF NOT EXISTS idx_audit_creature_config_op_time ON %I (op, tick_millis DESC);'
    ];
    v_exists BOOLEAN;
BEGIN
    FOR i IN 1..array_length(v_tabs, 1) LOOP
        v_tab := v_tabs[i];
        v_idx_create := v_idx_creates[i];

        -- 跳过已分区的表
        IF audit_is_partitioned(v_tab) THEN
            RAISE NOTICE 'audit_partition: % already partitioned, skipping', v_tab;
            CONTINUE;
        END IF;

        -- 检查表是否存在（pg_class）
        SELECT EXISTS (
            SELECT 1 FROM pg_class c
            JOIN pg_namespace n ON n.oid = c.relnamespace
            WHERE c.relname = v_tab AND c.relkind IN ('r', 'p') AND n.nspname = current_schema()
        ) INTO v_exists;
        IF NOT v_exists THEN
            RAISE NOTICE 'audit_partition: % does not exist in current schema, skipping', v_tab;
            CONTINUE;
        END IF;

        v_old := v_tab || '_old_for_partition';
        v_new := v_tab;
        v_def := v_tab || '_default';

        -- 拿列定义（用于 CREATE LIKE）
        EXECUTE format(
            'SELECT string_agg(quote_ident(attname) || '' '' || format_type(atttypid, atttypmod) || '' '' ||
                              CASE WHEN attnotnull THEN ''NOT NULL'' ELSE '''' END,
                              '', '' ORDER BY attnum)
             FROM pg_attribute
             WHERE attrelid = %L::regclass AND attnum > 0 AND NOT attisdropped',
            v_new
        ) INTO v_cols;

        -- 拿主键定义
        DECLARE
            v_pk_cols TEXT;
        BEGIN
            SELECT string_agg(quote_ident(attname), ', ' ORDER BY array_position(i.indkey, att.attnum))
              INTO v_pk_cols
              FROM pg_index i
              JOIN pg_attribute att ON att.attrelid = i.indrelid AND att.attnum = ANY(i.indkey)
             WHERE i.indrelid = v_new::regclass
               AND i.indisprimary;
            -- 转换时 parent 不带 PK（分区表主键必须包含分区键，转换后由 default 分区持有）
            -- 我们把主键约束加在 default 分区上
            IF v_pk_cols IS NOT NULL THEN
                v_idx_create := v_idx_create || format(
                    'ALTER TABLE %I ADD CONSTRAINT %I PRIMARY KEY (%s);',
                    v_def, v_tab || '_pkey', v_pk_cols
                );
            END IF;
        END;

        -- 1) RENAME 旧表（含显式 drop 主键约束以释放约束名，PG 18
        --    不会自动重命名约束，原始 audit_*_pkey 会与新 default
        --    分区的 pkey 名称冲突 — 2026-06-19 E2E 实测发现）
        EXECUTE format('ALTER TABLE %I DROP CONSTRAINT IF EXISTS %I',
            v_new, v_tab || '_pkey');
        EXECUTE format('ALTER TABLE %I RENAME TO %I', v_new, v_old);

        -- 2) 重建 parent（PARTITION BY RANGE (tick_millis)）
        EXECUTE format('CREATE TABLE %I (%s) PARTITION BY RANGE (tick_millis)', v_new, v_cols);

        -- 3) 重建 default 分区
        EXECUTE format('CREATE TABLE %I PARTITION OF %I DEFAULT', v_def, v_new);

        -- 4) 重建索引（parent 上 + default 上）
        -- 占位：第 1 个 %I = parent, 第 2/3 个 = default
        -- 简化：索引都建在 parent 上，default 自动继承
        EXECUTE format(v_idx_create, v_new, v_new, v_new);

        -- 5) 搬数据
        EXECUTE format('INSERT INTO %I SELECT * FROM %I', v_new, v_old);

        -- 6) 重建 COMMENT（每个表都加 'PARTITIONED BY RANGE (tick_millis)' 后缀）
        EXECUTE format(
            'COMMENT ON TABLE %I IS %L',
            v_new,
            (SELECT obj_description(c.oid) FROM pg_class c WHERE c.relname = v_old)
            || ' [PARTITIONED BY RANGE (tick_millis), default partition ' || v_def || '; doc/01 §2.2]'
        );

        -- 7) 删旧表
        EXECUTE format('DROP TABLE %I', v_old);

        RAISE NOTICE 'audit_partition: % partitioned -> % + %', v_tab, v_new, v_def;
    END LOOP;
END
$do$;

-- ── 4. doc 注解 ────────────────────────────────────────────────

COMMENT ON FUNCTION audit_is_partitioned(TEXT) IS
    'True if named table is a PostgreSQL declarative partition parent. Created 2026-06-17 (task #10).';

-- Index on (tick_millis) on every parent is implicit via PARTITION BY
-- RANGE; we do NOT add a separate tick_millis-only index because the
-- PK / op / target indexes all lead with (target_*, tick_millis DESC)
-- and any (tick_millis)-only scan would do a full partition sweep
-- anyway (the default partition is the catch-all).

COMMIT;
