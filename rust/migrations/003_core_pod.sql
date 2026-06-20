-- 2026-06-14 user task #5 — core-pod production (04-core-pod.md)
--
-- Adds two tables:
--   1. core_pods       — durable per-pod state (the 5-tuple primary key).
--   2. audit_core_pod  — append-only audit trail for every gRPC RPC
--                        (TickPod / EnterPod / ExitPod / GetPodState).
--
-- Mirrors 04 §6.3 (PostgreSQL 表) and 99 §5 (PostgreSQL 表映射 →
-- 04-core-pod → core_pods). The audit table extends the spec to also
-- cover the JNI direct-call entry `computePodStress` (logged as
-- `pod.stress_compute` from the gRPC layer when a tick round-trips
-- through Sable — the in-process direct call itself does not write
-- here, see doc/16-sable-bridge.md §3.4).

CREATE TABLE IF NOT EXISTS core_pods (
  world_uuid        UUID         NOT NULL,
  dimension         VARCHAR(64)  NOT NULL,
  pos_x             BIGINT       NOT NULL,
  pos_y             BIGINT       NOT NULL,
  pos_z             BIGINT       NOT NULL,
  host_uuid         UUID,
  endurance         FLOAT        NOT NULL DEFAULT 100.0
                                  CHECK (endurance >= 0 AND endurance <= 100),
  recipe_cooldown   BIGINT       NOT NULL DEFAULT 0,
  input_fluid_id    VARCHAR(128),
  input_fluid_mb    INT          NOT NULL DEFAULT 0,
  output_fluid_id   VARCHAR(128),
  output_fluid_mb   INT          NOT NULL DEFAULT 0,
  byproduct_count   BIGINT       NOT NULL DEFAULT 0,
  created_tick      BIGINT       NOT NULL,
  updated_tick      BIGINT       NOT NULL,
  PRIMARY KEY (world_uuid, dimension, pos_x, pos_y, pos_z)
);

-- Index: "find all pods the player is currently hosting".
CREATE INDEX IF NOT EXISTS idx_core_pods_host
  ON core_pods(host_uuid)
  WHERE host_uuid IS NOT NULL;

-- Index: "find the most-recently-updated pods in a chunk" (chunk
-- re-save strategy — see 04 §7 性能影响).
CREATE INDEX IF NOT EXISTS idx_core_pods_updated
  ON core_pods(updated_tick DESC);

CREATE TABLE IF NOT EXISTS audit_core_pod (
  log_id                       UUID         PRIMARY KEY,
  actor_uuid                   UUID         NOT NULL,
  actor_type                   VARCHAR(16)  NOT NULL
                                            CHECK (actor_type IN (
                                              'PLAYER', 'ADMIN_CMD', 'RUST_SERVICE'
                                            )),
  target_pod_world_uuid        UUID         NOT NULL,
  target_pod_dimension         VARCHAR(64)  NOT NULL,
  target_pod_pos_x             BIGINT       NOT NULL,
  target_pod_pos_y             BIGINT       NOT NULL,
  target_pod_pos_z             BIGINT       NOT NULL,
  op                           VARCHAR(32)  NOT NULL
                                            CHECK (op IN (
                                              'pod.tick',
                                              'pod.enter',
                                              'pod.exit',
                                              'pod.stress_compute',
                                              'pod.produce'
                                            )),
  stress_units                 FLOAT,
  rpm                          FLOAT,
  input_fluid_mb               INT,
  output_fluid_mb              INT,
  byproduct_count              BIGINT,
  endurance_after              FLOAT,
  tick_millis                  BIGINT       NOT NULL,
  request_id                   UUID,
  notes                        JSONB
);

-- Index: per-pod recent history (powers the KubeJS onCorePodProduce
-- replay timeline — 04 §8).
CREATE INDEX IF NOT EXISTS idx_audit_core_pod_target_time
  ON audit_core_pod(
    target_pod_world_uuid,
    target_pod_dimension,
    target_pod_pos_x,
    target_pod_pos_y,
    target_pod_pos_z,
    tick_millis DESC
  );

-- Index: op-typed scans (audit replay / heatmap dashboards).
CREATE INDEX IF NOT EXISTS idx_audit_core_pod_op_time
  ON audit_core_pod(op, tick_millis DESC);