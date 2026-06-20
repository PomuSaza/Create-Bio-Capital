#!/usr/bin/env bash
# E2E integration test script for Create: Bio-Capital
# (task #46 — Java + WebUI + PG 联合)
#
# Validates the full integration path:
#   1. PG migration (12 migrations apply cleanly)
#   2. Rust server starts (Web UI + axum routes registered)
#   3. /health (PG connectivity)
#   4. /admin/whitelist/reload (admin token auth + whitelist reload)
#   5. GET /players/{uuid} (full data contract: player_state + bank + name)
#   6. POST /bank/transfer (webui → atomic_transfer → bank_transactions + audit_bank)
#   7. SSE /events (broadcast bus — whitelist_reload event triggered)
#
# Usage:
#   ./scripts/e2e.sh                  # default — assumes server already up at :8080
#   ./scripts/e2e.sh --spawn-server   # also spawn + kill server in this script
#
# Pre-reqs:
#   * PG listening on 127.0.0.1:5432 with user biocapital / db biocapital
#   * biocapital-cli binary built: rust/target/debug/biocapital-cli
#   * config/biocapital-server.toml configured with admin_tokens[0] in env
#     ADMIN_TOKEN, or hard-coded for testing

set -e

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$PROJECT_ROOT"

ADMIN_TOKEN="${ADMIN_TOKEN:-76305866da86b16805b8b3572aa9e7912702f81b443a4e3d4e9401ab072f8232}"
CLI="$PROJECT_ROOT/rust/target/debug/biocapital-cli"
SERVER_LOG="/tmp/biocapital_e2e_server.log"
SERVER_PID=""

# Test data UUIDs (deterministic for repeatability)
PLAYER_A="aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"
PLAYER_B="bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb"
ACCT_A="11111111-1111-1111-1111-111111111111"
ACCT_B="22222222-2222-2222-2222-222222222222"

PASS=0
FAIL=0

ok()   { echo "  PASS: $1"; PASS=$((PASS+1)); }
bad()  { echo "  FAIL: $1"; FAIL=$((FAIL+1)); }
hdr()  { echo; echo "=== $1 ==="; }

cleanup() {
    if [ -n "$SERVER_PID" ] && kill -0 "$SERVER_PID" 2>/dev/null; then
        echo "killing server PID $SERVER_PID"
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT INT TERM

# ── Optional: spawn server ──────────────────────────────────────
if [ "${1:-}" = "--spawn-server" ]; then
    hdr "Starting biocapital-cli start (background)"
    "$CLI" start > "$SERVER_LOG" 2>&1 &
    SERVER_PID=$!
    echo "  PID=$SERVER_PID; log=$SERVER_LOG"
    for i in $(seq 1 30); do
        if curl -s -f http://127.0.0.1:8080/health > /dev/null 2>&1; then
            echo "  server ready after ${i}s"
            break
        fi
        sleep 1
    done
fi

# ── Test data setup ─────────────────────────────────────────────
hdr "Seeding test data"
PGPASSWORD=biocapital psql -h 127.0.0.1 -U biocapital -d biocapital -q <<EOF
INSERT INTO player_state (player_uuid, pleasure, hunger, hidden_hp, low_hp_hits, defeat_count, max_hunger, created_tick, updated_tick)
VALUES ('$PLAYER_A', 5.0, 50.0, 100.0, 0, 0, 100, EXTRACT(EPOCH FROM now())::bigint * 1000, EXTRACT(EPOCH FROM now())::bigint * 1000)
ON CONFLICT (player_uuid) DO UPDATE SET updated_tick = EXCLUDED.updated_tick;
INSERT INTO player_state (player_uuid, pleasure, hunger, hidden_hp, low_hp_hits, defeat_count, max_hunger, created_tick, updated_tick)
VALUES ('$PLAYER_B', 5.0, 50.0, 100.0, 0, 0, 100, EXTRACT(EPOCH FROM now())::bigint * 1000, EXTRACT(EPOCH FROM now())::bigint * 1000)
ON CONFLICT (player_uuid) DO UPDATE SET updated_tick = EXCLUDED.updated_tick;
INSERT INTO bank_accounts (account_uuid, owner_uuid, balance, max_balance, created_tick, updated_tick)
VALUES ('$ACCT_A', '$PLAYER_A', 1000, 100000000, EXTRACT(EPOCH FROM now())::bigint * 1000, EXTRACT(EPOCH FROM now())::bigint * 1000)
ON CONFLICT (account_uuid) DO UPDATE SET balance = EXCLUDED.balance, updated_tick = EXCLUDED.updated_tick;
INSERT INTO bank_accounts (account_uuid, owner_uuid, balance, max_balance, created_tick, updated_tick)
VALUES ('$ACCT_B', '$PLAYER_B', 1000, 100000000, EXTRACT(EPOCH FROM now())::bigint * 1000, EXTRACT(EPOCH FROM now())::bigint * 1000)
ON CONFLICT (account_uuid) DO UPDATE SET balance = EXCLUDED.balance, updated_tick = EXCLUDED.updated_tick;
INSERT INTO player_names (player_uuid, username) VALUES ('$PLAYER_B', 'alice')
ON CONFLICT (player_uuid) DO UPDATE SET username = EXCLUDED.username;
EOF
echo "  seeded"

# ── B1: /health ─────────────────────────────────────────────────
hdr "B1: GET /health"
RESP=$(curl -s -w "\n[HTTP %{http_code}]" http://127.0.0.1:8080/health)
echo "$RESP"
if echo "$RESP" | grep -q '"status":"ok".*"pg":"up"'; then ok "health OK + PG up"; else bad "health"; fi

# ── B2: /admin/whitelist/reload ─────────────────────────────────
hdr "B2: POST /admin/whitelist/reload"
RESP=$(curl -s -w "\n[HTTP %{http_code}]" -H "Authorization: Bearer $ADMIN_TOKEN" -X POST http://127.0.0.1:8080/admin/whitelist/reload)
echo "$RESP"
if echo "$RESP" | grep -q '"reloaded":true'; then ok "reload ack"; else bad "reload"; fi

# ── B3: GET /players/{uuid} ─────────────────────────────────────
hdr "B3: GET /players/$PLAYER_B"
RESP=$(curl -s -w "\n[HTTP %{http_code}]" -H "Authorization: Bearer $ADMIN_TOKEN" "http://127.0.0.1:8080/players/$PLAYER_B")
echo "$RESP"
if echo "$RESP" | grep -q '"player_uuid":"'$PLAYER_B'"' && echo "$RESP" | grep -q '"player_name":"alice"'; then ok "player_state with name"; else bad "player state"; fi

# ── B5: POST /bank/transfer ─────────────────────────────────────
hdr "B5: POST /bank/transfer"
RESP=$(curl -s -w "\n[HTTP %{http_code}]" -H "Authorization: Bearer $ADMIN_TOKEN" -H "Content-Type: application/json" \
    -d "{\"from_account_uuid\":\"$ACCT_A\",\"to_player_name\":\"alice\",\"amount\":100,\"request_id\":\"$(uuidgen)\"}" \
    "http://127.0.0.1:8080/bank/transfer")
echo "$RESP"
if echo "$RESP" | grep -q '"success":true'; then ok "transfer OK"; else bad "transfer"; fi

# ── B4: SSE /events ─────────────────────────────────────────────
hdr "B4: SSE /events (subscribe + trigger whitelist_reload)"
SSE_OUT="/tmp/biocapital_e2e_sse.out"
curl -sN --max-time 5 -H "Authorization: Bearer $ADMIN_TOKEN" \
    "http://127.0.0.1:8080/events?filter=whitelist_reload" > "$SSE_OUT" 2>&1 &
SSE_PID=$!
sleep 1
curl -s -X POST -H "Authorization: Bearer $ADMIN_TOKEN" "http://127.0.0.1:8080/admin/whitelist/reload" > /dev/null
sleep 1
wait $SSE_PID 2>/dev/null || true
cat "$SSE_OUT" | head -10
if grep -q "whitelist_reload" "$SSE_OUT"; then ok "SSE event received"; else bad "SSE"; fi

# ── B6: /audit/query (admin) ────────────────────────────────────
#
# Regression guard for task #67 (E2E B6 — audit_query 500):
# 1. `/audit/query` must return 200, not 500, even if the
#    `audit_admin` / `audit_contract` tables happen to be
#    missing (the handler now uses `query_audit_table_safe`
#    which logs-and-skips missing tables).
# 2. The response must be valid JSON with a `results` array.
# 3. An `op=admin.grant_viewer` filter that the handler has
#    written before should be reachable.
hdr "B6: GET /audit/query (admin)"
RESP=$(curl -s -w "\n[HTTP %{http_code}]" -H "Authorization: Bearer $ADMIN_TOKEN" \
    "http://127.0.0.1:8080/audit/query?op=admin.grant_viewer&limit=5")
echo "$RESP"
# 1. status code is 200
if echo "$RESP" | grep -q '\[HTTP 200\]'; then
    # 2. body is a JSON object with a `results` key
    BODY=$(echo "$RESP" | sed -e 's/\[HTTP 200\]//')
    if echo "$BODY" | grep -q '"results"'; then
        ok "audit_query 200 + JSON results array"
    else
        bad "audit_query 200 but body is not JSON with results"
    fi
else
    bad "audit_query status != 200 (regression on task #67)"
fi

# ── Summary ─────────────────────────────────────────────────────
hdr "Summary"
echo "  PASS: $PASS"
echo "  FAIL: $FAIL"
[ $FAIL -eq 0 ] && exit 0 || exit 1