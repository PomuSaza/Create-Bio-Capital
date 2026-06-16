//! gRPC service for `BankService` (`doc/14-rust-services.md` §3.2).
//!
//! Proto path: `rust/proto/biocapital.proto` → `biocapital.v1` package.
//!
//! Nine RPCs are implemented:
//!   - `GetBalance`           (method_id 0)
//!   - `Deposit`              (method_id 1)
//!   - `Withdraw`             (method_id 2)
//!   - `Transfer`             (method_id 3)
//!   - `GetHistory`           (method_id 4)
//!   - `LockDevice`           (method_id 5)
//!   - `UnlockDevice`         (method_id 6)
//!   - `GenerateInviteCode`   (method_id 7)
//!   - `AcceptInviteCode`     (method_id 8)
//!
//! Every mutating RPC writes an entry to the `audit_bank` table
//! (see `biocapital-pg` migration `20260614000002_bank.sql`) and
//! emits a `BankTransactionEvent` / `BankCardDeviceLockChangeEvent`
//! via the response's `event_meta` field. The Java side (Sable
//! bridge) reads the event and fires the corresponding NeoForge
//! event — see `doc/14-rust-services.md` §3.6 and
//! `doc/16-sable-bridge.md` §3.4.

use std::sync::Arc;

use chrono::Utc;
use tonic::{Request, Response, Status};
use uuid::Uuid;

// `BankOp` is only referenced by the test module below;
// `use super::*` re-exports the lib's top-level imports.
#[allow(unused_imports)]
use biocapital_bank::domain::{
    validate_deposit, validate_transfer, validate_withdraw, BankOp, BankTransaction,
    HardwareToken, HardwareTokenStatus, Whitelist, MAX_BALANCE,
};
// `BankAuditWriter` is only referenced by the test module below,
// but `use super::*` in `mod tests` re-exports the lib's top-level
// imports, so we keep it here under an explicit `#[allow]` rather
// than re-importing in the test module.
#[allow(unused_imports)]
use biocapital_pg::{
    BankAuditEntry, BankAuditWriter, BankRepository, BankServiceDeps, BankRepoError,
    HardwareTokenAuditEntry, HardwareTokenAuditWriter, HardwareTokenRepository,
    HardwareTokenRepoError, HardwareTokenServiceDeps,
};

// ── Opaque request/response types (proto-shaped) ───────────────────────────
//
// The build script that wires `prost-build` is part of task #17
// follow-up; until then each RPC signature accepts a request that
// exposes the proto `PlayerIdentifier.player_uuid` and the per-RPC
// fields, and returns a proto-shaped `BalanceResponse`. The
// `BankRpc` trait is the integration boundary; it stays close to
// the proto shape so the swap-in of generated types is mechanical.

#[derive(Debug, Clone, Default)]
pub struct PlayerIdentifier {
    pub player_uuid: Uuid,
}

#[derive(Debug, Clone, Default)]
pub struct AccountRequest {
    pub player: PlayerIdentifier,
    pub device_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BalanceResponse {
    pub balance: i64,
    pub max_balance: i64,
    pub device_locked: bool,
    pub account_uuid: Uuid,
    /// Cross-RPC event payload the Sable JNI bridge translates into
    /// a NeoForge event. Always present on mutation; absent (or
    /// `kind = "none"`) on read-only queries.
    pub event_meta: EventMeta,
}

#[derive(Debug, Clone)]
pub struct DepositRequest {
    pub player: PlayerIdentifier,
    pub amount: i64,
    pub batch_id: Uuid,
    pub request_id: Uuid,
}

#[derive(Debug, Clone)]
pub struct WithdrawRequest {
    pub player: PlayerIdentifier,
    pub amount: i64,
    pub request_id: Uuid,
}

#[derive(Debug, Clone)]
pub struct TransferRequest {
    pub from: PlayerIdentifier,
    pub to: PlayerIdentifier,
    pub amount: i64,
    pub memo: Option<String>,
    pub request_id: Uuid,
}

#[derive(Debug, Clone, Default)]
pub struct HistoryRequest {
    pub player: PlayerIdentifier,
    pub limit: i32,
    pub before_tick_millis: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct HistoryResponse {
    pub entries: Vec<BankTransaction>,
    pub next_before_tick_millis: i64,
}

#[derive(Debug, Clone)]
pub struct LockDeviceRequest {
    pub player: PlayerIdentifier,
    pub device_id: String,
}

#[derive(Debug, Clone)]
pub struct LockDeviceResponse {
    pub success: bool,
    pub already_locked: bool,
    pub current_device_id: Option<String>,
    pub event_meta: EventMeta,
}

#[derive(Debug, Clone)]
pub struct UnlockDeviceRequest {
    pub player: PlayerIdentifier,
    pub device_id: String,
    pub invite_code: String,
}

#[derive(Debug, Clone)]
pub struct InviteCodeResponse {
    pub invite_code: String,
    pub expires_at: chrono::DateTime<Utc>,
    pub event_meta: EventMeta,
}

#[derive(Debug, Clone)]
pub struct AcceptInviteRequest {
    pub player: PlayerIdentifier,
    pub invite_code: String,
}

// ── 硬件 token 子系统 (doc/18 §7) ──────────────────────────────────────────
//
// 5 个新 RPC：
//   - RequestHardwareToken (method_id 9)
//   - BindHardware         (method_id 10)
//   - ListHardware         (method_id 11)
//   - RevokeHardware       (method_id 12)
//   - Authenticate         (method_id 13)
//
// TokenResponse / TokenListResponse 在 DglabService 块（proto 行 461-474）
// 已经定义；这里只做 Rust-side 镜像（参照 dglab_service.rs 的同款结构）。
// 18 §7 的复用约定是 proto 层复用 BankService 5 个新 RPC 的
// request/response，Rust 层在 gRPC dispatch 之前先做转换。

#[derive(Debug, Clone)]
pub struct TokenResponse {
    /// UUIDv4 (proto `string token` for 18 §7 compat; we keep
    /// the canonical `Uuid` in Rust)
    pub token: Uuid,
    pub owner_uuid: Uuid,
    pub created_tick: i64,
    pub last_used_tick: i64,
    pub enabled: bool,
    pub event_meta: EventMeta,
}

#[derive(Debug, Clone)]
pub struct TokenListResponse {
    pub tokens: Vec<TokenResponse>,
    pub total_count: i32,
    pub event_meta: EventMeta,
}

#[derive(Debug, Clone)]
pub struct BindHardwareRequest {
    pub owner_uuid: Uuid,
    /// 52 chars base32 (18 §4.2)
    pub hardware_id_hash: String,
    /// UUIDv4 token, minted by RequestHardwareToken
    pub token: Uuid,
}

#[derive(Debug, Clone)]
pub struct RevokeHardwareRequest {
    pub owner_uuid: Uuid,
    pub token_id: Uuid,
}

#[derive(Debug, Clone)]
pub struct AuthenticateRequest {
    pub player_uuid: Uuid,
    pub player_username: String,
    /// 52 chars base32 — Java 端 HardwareIdCollector 自动采集
    pub hardware_id_hash: String,
}

#[derive(Debug, Clone)]
pub struct AuthenticateResponse {
    pub allowed: bool,
    /// 18 §7 wire values:
    /// "whitelisted" | "hardware_token_valid" |
    /// "no_hardware_token" | "hardware_token_expired" |
    /// "hardware_id_mismatch"
    pub reason: String,
    pub event_meta: EventMeta,
}

// ── Event meta ─────────────────────────────────────────────────────────────
//
// Sable JNI consumes this to fire the NeoForge events listed in
// 99 §3.1 (`BankTransactionEvent`, `BankCardDeviceLockChangeEvent`).
// The Java side translates the `kind` string into the matching
// `Event` subclass.

#[derive(Debug, Clone, Default)]
pub struct EventMeta {
    /// One of: "none", "bank.tx", "card.lock", "card.unlock", "card.invite".
    pub kind: String,
    /// Opaque payload serialised to JSONB by the Java side. The
    /// fields are documented per RPC in the dispatch table at the
    /// top of this module.
    pub payload_json: String,
}

// ── gRPC service trait ──────────────────────────────────────────────────────

#[tonic::async_trait]
pub trait BankRpc: Send + Sync + 'static {
    async fn get_balance(
        &self,
        request: Request<AccountRequest>,
    ) -> Result<Response<BalanceResponse>, Status>;

    async fn deposit(
        &self,
        request: Request<DepositRequest>,
    ) -> Result<Response<BalanceResponse>, Status>;

    async fn withdraw(
        &self,
        request: Request<WithdrawRequest>,
    ) -> Result<Response<BalanceResponse>, Status>;

    async fn transfer(
        &self,
        request: Request<TransferRequest>,
    ) -> Result<Response<BalanceResponse>, Status>;

    async fn get_history(
        &self,
        request: Request<HistoryRequest>,
    ) -> Result<Response<HistoryResponse>, Status>;

    async fn lock_device(
        &self,
        request: Request<LockDeviceRequest>,
    ) -> Result<Response<LockDeviceResponse>, Status>;

    async fn unlock_device(
        &self,
        request: Request<UnlockDeviceRequest>,
    ) -> Result<Response<LockDeviceResponse>, Status>;

    async fn generate_invite_code(
        &self,
        request: Request<AccountRequest>,
    ) -> Result<Response<InviteCodeResponse>, Status>;

    async fn accept_invite_code(
        &self,
        request: Request<AcceptInviteRequest>,
    ) -> Result<Response<LockDeviceResponse>, Status>;

    // ── 硬件 token 子系统 (doc/18 §7) ─────────────────────────

    async fn request_hardware_token(
        &self,
        request: Request<AccountRequest>,
    ) -> Result<Response<TokenResponse>, Status>;

    async fn bind_hardware(
        &self,
        request: Request<BindHardwareRequest>,
    ) -> Result<Response<TokenResponse>, Status>;

    async fn list_hardware(
        &self,
        request: Request<AccountRequest>,
    ) -> Result<Response<TokenListResponse>, Status>;

    async fn revoke_hardware(
        &self,
        request: Request<RevokeHardwareRequest>,
    ) -> Result<Response<TokenResponse>, Status>;

    async fn authenticate(
        &self,
        request: Request<AuthenticateRequest>,
    ) -> Result<Response<AuthenticateResponse>, Status>;
}

// ── Implementation ──────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct BankGrpc {
    deps: BankServiceDeps,
    /// 硬件 token 子系统 (doc/18 §3.2 + §5) — 仅在 Request/Bind/List/Revoke
    /// 5 个新 RPC 与 Authenticate 流程上启用；其余 9 个旧 RPC 不依赖。
    hw_deps: HardwareTokenServiceDeps,
    /// 内存 cache（启动加载 + SIGHUP / file-notify 热重载）。冷启动时
    /// 是 empty whitelist（永不命中）。Authenticate 热路径读这个。
    whitelist: Arc<std::sync::RwLock<Whitelist>>,
    /// Logical clock. Default: `Utc::now().timestamp_millis()`.
    tick_millis: Arc<dyn Fn() -> i64 + Send + Sync>,
    /// Invite-code expiry window (12 §2.7 — 10 minutes default).
    invite_code_ttl_secs: i64,
}

impl BankGrpc {
    /// Backward-compatible ctor: 仅 bank deps，无 hardware-token 支持。
    /// Authenticate 5 个新 RPC 在这种 service 上调会返回 UNIMPLEMENTED 语义错误。
    /// **不推荐**生产使用 — 应使用 [`BankGrpc::with_hardware_token`]。
    pub fn new(deps: BankServiceDeps) -> Self {
        Self {
            deps,
            hw_deps: hardware_token_stub_deps(),
            whitelist: Arc::new(std::sync::RwLock::new(Whitelist::empty())),
            tick_millis: Arc::new(|| Utc::now().timestamp_millis()),
            invite_code_ttl_secs: 600,
        }
    }

    /// Backward-compatible ctor with custom clock.
    pub fn with_clock(
        deps: BankServiceDeps,
        clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    ) -> Self {
        Self {
            deps,
            hw_deps: hardware_token_stub_deps(),
            whitelist: Arc::new(std::sync::RwLock::new(Whitelist::empty())),
            tick_millis: clock,
            invite_code_ttl_secs: 600,
        }
    }

    /// 标准 ctor（task #83 之后推荐）。`BankServiceDeps` + 硬件 token 子系统 +
    /// 初始 whitelist + 默认邀请码 TTL。
    pub fn with_hardware_token(
        deps: BankServiceDeps,
        hw_deps: HardwareTokenServiceDeps,
        whitelist: Whitelist,
    ) -> Self {
        Self {
            deps,
            hw_deps,
            whitelist: Arc::new(std::sync::RwLock::new(whitelist)),
            tick_millis: Arc::new(|| Utc::now().timestamp_millis()),
            invite_code_ttl_secs: 600,
        }
    }

    /// 运行时热重载 whitelist（18 §2.2 "SIGHUP / file-notify"）。
    /// 读路径（`Authenticate`）持读锁；写路径（reload）持写锁 —
    /// 短临界区，新白名单原子换入。
    pub fn reload_whitelist(&self, new: Whitelist) {
        if let Ok(mut w) = self.whitelist.write() {
            *w = new;
        }
    }

    /// 周期 sweep 入口（18 §3.3）。返回本轮被标记过期的 row 数，
    /// gRPC 进程 / Rust CLI 可 metric 化。
    pub async fn sweep_expire_overdue(&self) -> Result<u64, Status> {
        let now = Utc::now();
        self.hw_deps
            .repo
            .expire_overdue(now)
            .await
            .map_err(hw_repo_status)
    }
}

/// 提供一个 no-op 的 `HardwareTokenServiceDeps`，让 `BankGrpc::new`
/// 这种纯-bank 的旧 ctor 仍然能编译。`expire_overdue` / 5 个新 RPC
/// 在这种 service 上调会返回 UNIMPLEMENTED。
fn hardware_token_stub_deps() -> HardwareTokenServiceDeps {
    use std::sync::Arc;

    struct StubRepo;
    #[async_trait::async_trait]
    impl HardwareTokenRepository for StubRepo {
        async fn create_token(
            &self,
            _: Uuid,
            _: Uuid,
            _: chrono::DateTime<Utc>,
            _: chrono::DateTime<Utc>,
        ) -> Result<HardwareToken, HardwareTokenRepoError> {
            Err(HardwareTokenRepoError::Sqlx(sqlx::Error::Protocol(
                "hardware_token subsystem not configured".into(),
            )))
        }
        async fn bind_hardware(
            &self,
            _: Uuid,
            _: Uuid,
            _: String,
        ) -> Result<HardwareToken, HardwareTokenRepoError> {
            Err(HardwareTokenRepoError::Sqlx(sqlx::Error::Protocol(
                "hardware_token subsystem not configured".into(),
            )))
        }
        async fn revoke_token(
            &self,
            _: Uuid,
            _: Uuid,
            _: chrono::DateTime<Utc>,
        ) -> Result<HardwareToken, HardwareTokenRepoError> {
            Err(HardwareTokenRepoError::Sqlx(sqlx::Error::Protocol(
                "hardware_token subsystem not configured".into(),
            )))
        }
        async fn list_for_owner(
            &self,
            _: Uuid,
            _: bool,
        ) -> Result<Vec<HardwareToken>, HardwareTokenRepoError> {
            Ok(Vec::new())
        }
        async fn find_valid_token(
            &self,
            _: Uuid,
            _: &str,
            _: chrono::DateTime<Utc>,
        ) -> Result<Option<HardwareToken>, HardwareTokenRepoError> {
            Ok(None)
        }
        async fn expire_overdue(
            &self,
            _: chrono::DateTime<Utc>,
        ) -> Result<u64, HardwareTokenRepoError> {
            Ok(0)
        }
    }
    struct StubAudit;
    #[async_trait::async_trait]
    impl HardwareTokenAuditWriter for StubAudit {
        async fn write(
            &self,
            _: HardwareTokenAuditEntry,
        ) -> Result<(), HardwareTokenRepoError> {
            // Drop the entry — stub does not persist.
            Ok(())
        }
    }
    HardwareTokenServiceDeps::new(
        Arc::new(StubRepo) as Arc<dyn HardwareTokenRepository>,
        Arc::new(StubAudit) as Arc<dyn HardwareTokenAuditWriter>,
    )
}

#[tonic::async_trait]
impl BankRpc for BankGrpc {
    // ── GetBalance ──────────────────────────────────────────────
    async fn get_balance(
        &self,
        request: Request<AccountRequest>,
    ) -> Result<Response<BalanceResponse>, Status> {
        let AccountRequest { player, device_id } = request.into_inner();
        // Read by owner_uuid — the durable index lookup. The
        // gRPC layer is happy to materialise a fresh default
        // account row when the player has never transacted.
        let account = match self.deps.repo.get_account_by_owner(player.player_uuid).await {
            Ok(a) => a,
            Err(BankRepoError::AccountNotFound { .. }) => {
                // No row yet — fabricate a default with a fresh
                // account_uuid and zero balance. The `is_locked_to`
                // check is trivially false on a never-seen player.
                return Ok(Response::new(BalanceResponse {
                    balance: 0,
                    max_balance: MAX_BALANCE,
                    device_locked: false,
                    account_uuid: Uuid::nil(),
                    event_meta: EventMeta::default(),
                }));
            }
            Err(e) => return Err(repo_status(e)),
        };

        let device_locked = account.is_locked_to(device_id.as_deref());
        Ok(Response::new(BalanceResponse {
            balance: account.balance,
            max_balance: account.max_balance,
            device_locked,
            account_uuid: account.account_uuid,
            event_meta: EventMeta::default(),
        }))
    }

    // ── Deposit ─────────────────────────────────────────────────
    async fn deposit(
        &self,
        request: Request<DepositRequest>,
    ) -> Result<Response<BalanceResponse>, Status> {
        let DepositRequest {
            player,
            amount,
            batch_id,
            request_id,
        } = request.into_inner();
        if amount <= 0 {
            return Err(Status::invalid_argument("amount must be positive"));
        }

        let account = ensure_account(
            &self.deps.repo,
            player.player_uuid,
            (self.tick_millis)(),
        )
        .await?;
        let before_balance = account.balance;
        let max_balance = account.max_balance;
        let _accepted =
            validate_deposit(&account, amount).map_err(domain_to_status)?;

        let tick = (self.tick_millis)();
        let tx = self
            .deps
            .repo
            .atomic_deposit(
                account.account_uuid,
                amount,
                batch_id,
                request_id,
                tick,
            )
            .await
            .map_err(repo_status)?;

        // Refresh after the atomic write.
        let after = self
            .deps
            .repo
            .get_account(account.account_uuid)
            .await
            .map_err(repo_status)?;

        self.deps
            .audit
            .write(BankAuditEntry {
                log_id: Uuid::new_v4(),
                actor_uuid: player.player_uuid,
                actor_type: "PLAYER",
                target_account_uuid: account.account_uuid,
                op: "bank.deposit",
                before_balance,
                after_balance: after.balance,
                tick_millis: tick,
                request_id: Some(request_id),
                notes: Some(serde_json::json!({
                    "batch_id": batch_id,
                    "tx_id": tx.tx_id,
                    "amount": tx.amount,
                })),
            })
            .await
            .map_err(repo_status)?;

        Ok(Response::new(BalanceResponse {
            balance: after.balance,
            max_balance,
            device_locked: after.is_locked_to(None),
            account_uuid: after.account_uuid,
            event_meta: event_meta_for_tx(&tx),
        }))
    }

    // ── Withdraw ───────────────────────────────────────────────
    async fn withdraw(
        &self,
        request: Request<WithdrawRequest>,
    ) -> Result<Response<BalanceResponse>, Status> {
        let WithdrawRequest { player, amount, request_id } = request.into_inner();
        if amount <= 0 {
            return Err(Status::invalid_argument("amount must be positive"));
        }

        let account = match self.deps.repo.get_account_by_owner(player.player_uuid).await {
            Ok(a) => a,
            Err(BankRepoError::AccountNotFound { .. }) => {
                return Ok(Response::new(BalanceResponse {
                    balance: 0,
                    max_balance: MAX_BALANCE,
                    device_locked: false,
                    account_uuid: Uuid::nil(),
                    event_meta: EventMeta::default(),
                }));
            }
            Err(e) => return Err(repo_status(e)),
        };
        let before_balance = account.balance;
        let max_balance = account.max_balance;
        // Clamp the requested amount to the actual balance so an
        // overdraw returns 0 rather than a `FailedPrecondition` error
        // (08 §3.4 rule 2 / panel 2 design intent — withdrawing more
        // than you have silently empties the account, never rejects).
        let amount = amount.min(account.balance);
        let _accepted = validate_withdraw(&account, amount).map_err(domain_to_status)?;

        let tick = (self.tick_millis)();
        let tx = self
            .deps
            .repo
            .atomic_withdraw(account.account_uuid, amount, request_id, tick)
            .await
            .map_err(repo_status)?;

        let after = self
            .deps
            .repo
            .get_account(account.account_uuid)
            .await
            .map_err(repo_status)?;

        self.deps
            .audit
            .write(BankAuditEntry {
                log_id: Uuid::new_v4(),
                actor_uuid: player.player_uuid,
                actor_type: "PLAYER",
                target_account_uuid: account.account_uuid,
                op: "bank.withdraw",
                before_balance,
                after_balance: after.balance,
                tick_millis: tick,
                request_id: Some(request_id),
                notes: Some(serde_json::json!({
                    "tx_id": tx.tx_id,
                    "amount": tx.amount,
                })),
            })
            .await
            .map_err(repo_status)?;

        Ok(Response::new(BalanceResponse {
            balance: after.balance,
            max_balance,
            device_locked: after.is_locked_to(None),
            account_uuid: after.account_uuid,
            event_meta: event_meta_for_tx(&tx),
        }))
    }

    // ── Transfer ───────────────────────────────────────────────
    async fn transfer(
        &self,
        request: Request<TransferRequest>,
    ) -> Result<Response<BalanceResponse>, Status> {
        let TransferRequest { from, to, amount, memo, request_id } = request.into_inner();
        if amount <= 0 {
            return Err(Status::invalid_argument("amount must be positive"));
        }
        if from.player_uuid == to.player_uuid {
            return Err(Status::invalid_argument("from == to"));
        }

        // Validate via the domain module so the rules (08 §3.4
        // 面板 2 + device lock) are enforced before any DB write.
        let from_account = ensure_account(
            &self.deps.repo,
            from.player_uuid,
            (self.tick_millis)(),
        )
        .await?;
        let _capped = validate_transfer(
            &from_account,
            amount,
            None,
            request_id,
        )
        .map_err(domain_to_status)?;

        // Ensure the destination exists (auto-create on first
        // inbound). Without this a transfer to a brand-new player
        // would fail the foreign-key check on `bank_accounts`.
        let _to_account = ensure_account(
            &self.deps.repo,
            to.player_uuid,
            (self.tick_millis)(),
        )
        .await?;

        let before_from = from_account.balance;
        let max_balance = from_account.max_balance;
        let tick = (self.tick_millis)();
        let (tx_out, _tx_in) = self
            .deps
            .repo
            .atomic_transfer(
                from_account.account_uuid,
                _to_account.account_uuid,
                amount,
                memo.clone(),
                request_id,
                tick,
            )
            .await
            .map_err(repo_status)?;

        let after = self
            .deps
            .repo
            .get_account(from_account.account_uuid)
            .await
            .map_err(repo_status)?;

        self.deps
            .audit
            .write(BankAuditEntry {
                log_id: Uuid::new_v4(),
                actor_uuid: from.player_uuid,
                actor_type: "PLAYER",
                target_account_uuid: from_account.account_uuid,
                op: "bank.transfer",
                before_balance: before_from,
                after_balance: after.balance,
                tick_millis: tick,
                request_id: Some(request_id),
                notes: Some(serde_json::json!({
                    "tx_id": tx_out.tx_id,
                    "to": to.player_uuid,
                    "amount": tx_out.amount,
                    "memo": memo,
                })),
            })
            .await
            .map_err(repo_status)?;

        Ok(Response::new(BalanceResponse {
            balance: after.balance,
            max_balance,
            device_locked: after.is_locked_to(None),
            account_uuid: after.account_uuid,
            event_meta: event_meta_for_tx(&tx_out),
        }))
    }

    // ── GetHistory ─────────────────────────────────────────────
    async fn get_history(
        &self,
        request: Request<HistoryRequest>,
    ) -> Result<Response<HistoryResponse>, Status> {
        let HistoryRequest { player, limit, before_tick_millis } = request.into_inner();
        let account = match self.deps.repo.get_account_by_owner(player.player_uuid).await {
            Ok(a) => a,
            Err(BankRepoError::AccountNotFound { .. }) => {
                return Ok(Response::new(HistoryResponse {
                    entries: Vec::new(),
                    next_before_tick_millis: -1,
                }));
            }
            Err(e) => return Err(repo_status(e)),
        };
        let limit = if limit <= 0 { 16 } else { limit.min(1024) };
        let mut entries = self
            .deps
            .repo
            .get_history(account.account_uuid, limit as i64)
            .await
            .map_err(repo_status)?;
        // The repository returns DESC; the proto's before_tick_millis
        // is a pagination cursor for "older than this". Filter to
        // entries strictly before the cursor if set.
        if let Some(cursor) = before_tick_millis {
            entries.retain(|e| e.tick_millis < cursor);
        }
        let next_before_tick_millis = entries
            .last()
            .map(|e| e.tick_millis)
            .unwrap_or(-1);
        Ok(Response::new(HistoryResponse {
            entries,
            next_before_tick_millis,
        }))
    }

    // ── LockDevice ─────────────────────────────────────────────
    async fn lock_device(
        &self,
        request: Request<LockDeviceRequest>,
    ) -> Result<Response<LockDeviceResponse>, Status> {
        let LockDeviceRequest { player, device_id } = request.into_inner();
        if device_id.is_empty() {
            return Err(Status::invalid_argument("device_id is empty"));
        }
        let account = ensure_account(
            &self.deps.repo,
            player.player_uuid,
            (self.tick_millis)(),
        )
        .await?;
        let already_locked = account.device_lock.is_some()
            && account.device_lock.as_deref() != Some(device_id.as_str());
        let before_lock = account.device_lock.clone();
        let updated = self
            .deps
            .repo
            .lock_device(account.account_uuid, device_id.clone())
            .await
            .map_err(repo_status)?;

        let tick = (self.tick_millis)();
        self.deps
            .audit
            .write(BankAuditEntry {
                log_id: Uuid::new_v4(),
                actor_uuid: player.player_uuid,
                actor_type: "PLAYER",
                target_account_uuid: account.account_uuid,
                op: "bank.lock_device",
                before_balance: updated.balance,
                after_balance: updated.balance,
                tick_millis: tick,
                request_id: None,
                notes: Some(serde_json::json!({
                    "device_id": device_id,
                    "before_lock": before_lock,
                    "already_locked": already_locked,
                })),
            })
            .await
            .map_err(repo_status)?;

        Ok(Response::new(LockDeviceResponse {
            success: true,
            already_locked,
            current_device_id: updated.device_lock.clone(),
            event_meta: EventMeta {
                kind: "card.lock".to_owned(),
                payload_json: serde_json::json!({
                    "account_uuid": updated.account_uuid,
                    "device_id": device_id,
                    "already_locked": already_locked,
                })
                .to_string(),
            },
        }))
    }

    // ── UnlockDevice ───────────────────────────────────────────
    async fn unlock_device(
        &self,
        request: Request<UnlockDeviceRequest>,
    ) -> Result<Response<LockDeviceResponse>, Status> {
        let UnlockDeviceRequest { player, device_id, invite_code } = request.into_inner();
        if device_id.is_empty() {
            return Err(Status::invalid_argument("device_id is empty"));
        }
        if invite_code.is_empty() {
            return Err(Status::invalid_argument("invite_code is empty"));
        }
        let account = ensure_account(
            &self.deps.repo,
            player.player_uuid,
            (self.tick_millis)(),
        )
        .await?;

        // Parse the invite code; on parse failure we treat it as
        // an invalid code. The real validation will land with the
        // /command-system task (#13) which adds a persistent
        // invite_codes table.
        let invite_uuid = Uuid::parse_str(&invite_code)
            .map_err(|_| Status::invalid_argument("invite_code is not a UUID"))?;

        let updated = self
            .deps
            .repo
            .unlock_device(account.account_uuid, invite_uuid)
            .await
            .map_err(repo_status)?;

        let tick = (self.tick_millis)();
        self.deps
            .audit
            .write(BankAuditEntry {
                log_id: Uuid::new_v4(),
                actor_uuid: player.player_uuid,
                actor_type: "PLAYER",
                target_account_uuid: account.account_uuid,
                op: "bank.unlock_device",
                before_balance: updated.balance,
                after_balance: updated.balance,
                tick_millis: tick,
                request_id: Some(invite_uuid),
                notes: Some(serde_json::json!({
                    "device_id": device_id,
                    "invite_code": invite_code,
                })),
            })
            .await
            .map_err(repo_status)?;

        Ok(Response::new(LockDeviceResponse {
            success: true,
            already_locked: false,
            current_device_id: updated.device_lock.clone(),
            event_meta: EventMeta {
                kind: "card.unlock".to_owned(),
                payload_json: serde_json::json!({
                    "account_uuid": updated.account_uuid,
                    "device_id": device_id,
                })
                .to_string(),
            },
        }))
    }

    // ── GenerateInviteCode ─────────────────────────────────────
    async fn generate_invite_code(
        &self,
        request: Request<AccountRequest>,
    ) -> Result<Response<InviteCodeResponse>, Status> {
        let AccountRequest { player, .. } = request.into_inner();
        let _account = ensure_account(
            &self.deps.repo,
            player.player_uuid,
            (self.tick_millis)(),
        )
        .await?;
        let code = Uuid::new_v4().to_string();
        let expires_at = Utc::now()
            + chrono::Duration::seconds(self.invite_code_ttl_secs);

        let tick = (self.tick_millis)();
        // The code is logged via the audit trail; a future invite_codes
        // table will own the lifetime / revocation.
        self.deps
            .audit
            .write(BankAuditEntry {
                log_id: Uuid::new_v4(),
                actor_uuid: player.player_uuid,
                actor_type: "PLAYER",
                target_account_uuid: _account.account_uuid,
                op: "bank.invite",
                before_balance: _account.balance,
                after_balance: _account.balance,
                tick_millis: tick,
                request_id: None,
                notes: Some(serde_json::json!({
                    "invite_code": code,
                    "expires_at": expires_at.to_rfc3339(),
                })),
            })
            .await
            .map_err(repo_status)?;

        Ok(Response::new(InviteCodeResponse {
            invite_code: code.clone(),
            expires_at,
            event_meta: EventMeta {
                kind: "card.invite".to_owned(),
                payload_json: serde_json::json!({
                    "invite_code": code,
                    "expires_at": expires_at.to_rfc3339(),
                })
                .to_string(),
            },
        }))
    }

    // ── AcceptInviteCode ───────────────────────────────────────
    async fn accept_invite_code(
        &self,
        request: Request<AcceptInviteRequest>,
    ) -> Result<Response<LockDeviceResponse>, Status> {
        let AcceptInviteRequest { player, invite_code } = request.into_inner();
        if invite_code.is_empty() {
            return Err(Status::invalid_argument("invite_code is empty"));
        }
        let account = ensure_account(
            &self.deps.repo,
            player.player_uuid,
            (self.tick_millis)(),
        )
        .await?;
        let invite_uuid = Uuid::parse_str(&invite_code)
            .map_err(|_| Status::invalid_argument("invite_code is not a UUID"))?;
        let updated = self
            .deps
            .repo
            .unlock_device(account.account_uuid, invite_uuid)
            .await
            .map_err(repo_status)?;

        let tick = (self.tick_millis)();
        self.deps
            .audit
            .write(BankAuditEntry {
                log_id: Uuid::new_v4(),
                actor_uuid: player.player_uuid,
                actor_type: "PLAYER",
                target_account_uuid: account.account_uuid,
                op: "bank.accept_invite",
                before_balance: updated.balance,
                after_balance: updated.balance,
                tick_millis: tick,
                request_id: Some(invite_uuid),
                notes: Some(serde_json::json!({
                    "invite_code": invite_code,
                })),
            })
            .await
            .map_err(repo_status)?;

        Ok(Response::new(LockDeviceResponse {
            success: true,
            already_locked: false,
            current_device_id: updated.device_lock.clone(),
            event_meta: EventMeta {
                kind: "card.unlock".to_owned(),
                payload_json: serde_json::json!({
                    "account_uuid": updated.account_uuid,
                    "via": "invite_code",
                })
                .to_string(),
            },
        }))
    }

    // ── 硬件 token 子系统（doc/18 §7）─────────────────────────

    // ── RequestHardwareToken ─────────────────────────────────
    async fn request_hardware_token(
        &self,
        request: Request<AccountRequest>,
    ) -> Result<Response<TokenResponse>, Status> {
        let AccountRequest { player, .. } = request.into_inner();
        let tick = (self.tick_millis)();
        let now = Utc::now();
        let token_id = Uuid::new_v4();
        let expires_at = HardwareToken::compute_expires_at(now);

        let token = self
            .hw_deps
            .repo
            .create_token(player.player_uuid, token_id, now, expires_at)
            .await
            .map_err(hw_repo_status)?;

        // Audit (18 §9.2). PG FIFO trigger may have flipped an
        // older row to REPLACED — the audit `op` is still
        // `token.request` for this call; the eviction is logged
        // separately as `token.fifo_evict` by the next caller's
        // `list_for_owner` sweep (or a future trigger-driven
        // audit hook).
        self.hw_deps
            .audit
            .write(HardwareTokenAuditEntry {
                log_id: Uuid::new_v4(),
                actor_uuid: Some(player.player_uuid),
                actor_type: "PLAYER",
                target_token_id: Some(token.token_id),
                target_owner_uuid: Some(token.owner_uuid),
                op: "token.request",
                hardware_id_hash: None,
                reason: None,
                tick_millis: tick,
                request_id: None,
            })
            .await
            .map_err(hw_repo_status)?;

        Ok(Response::new(hw_token_to_response(&token, "hw.issued")))
    }

    // ── BindHardware ────────────────────────────────────────
    async fn bind_hardware(
        &self,
        request: Request<BindHardwareRequest>,
    ) -> Result<Response<TokenResponse>, Status> {
        let BindHardwareRequest {
            owner_uuid,
            hardware_id_hash,
            token,
        } = request.into_inner();
        if hardware_id_hash.len() != 52 {
            return Err(Status::invalid_argument(
                "hardware_id_hash must be 52 base32 chars",
            ));
        }

        let tick = (self.tick_millis)();
        let updated = self
            .hw_deps
            .repo
            .bind_hardware(token, owner_uuid, hardware_id_hash.clone())
            .await
            .map_err(hw_repo_status)?;

        self.hw_deps
            .audit
            .write(HardwareTokenAuditEntry {
                log_id: Uuid::new_v4(),
                actor_uuid: Some(owner_uuid),
                actor_type: "PLAYER",
                target_token_id: Some(updated.token_id),
                target_owner_uuid: Some(updated.owner_uuid),
                op: "token.bind",
                hardware_id_hash: Some(hardware_id_hash),
                reason: None,
                tick_millis: tick,
                request_id: None,
            })
            .await
            .map_err(hw_repo_status)?;

        Ok(Response::new(hw_token_to_response(&updated, "hw.bound")))
    }

    // ── ListHardware ────────────────────────────────────────
    async fn list_hardware(
        &self,
        request: Request<AccountRequest>,
    ) -> Result<Response<TokenListResponse>, Status> {
        let AccountRequest { player, .. } = request.into_inner();
        // 18 §7 / 99 §3.1 `ListHardware` returns ALL tokens
        // (including REPLACED / EXPIRED / REVOKED) so the Web UI
        // can show history. The `Authenticate` hot path uses
        // `find_valid_token` directly — never this list.
        let rows = self
            .hw_deps
            .repo
            .list_for_owner(player.player_uuid, true)
            .await
            .map_err(hw_repo_status)?;
        let total = rows.len() as i32;
        let tokens = rows
            .iter()
            .map(|t| hw_token_to_response(t, "hw.listed"))
            .collect();

        Ok(Response::new(TokenListResponse {
            tokens,
            total_count: total,
            event_meta: EventMeta {
                kind: "hw.listed".to_owned(),
                payload_json: serde_json::json!({
                    "owner_uuid": player.player_uuid,
                    "total_count": total,
                })
                .to_string(),
            },
        }))
    }

    // ── RevokeHardware ──────────────────────────────────────
    async fn revoke_hardware(
        &self,
        request: Request<RevokeHardwareRequest>,
    ) -> Result<Response<TokenResponse>, Status> {
        let RevokeHardwareRequest {
            owner_uuid,
            token_id,
        } = request.into_inner();
        let now = Utc::now();
        let tick = (self.tick_millis)();

        let updated = self
            .hw_deps
            .repo
            .revoke_token(token_id, owner_uuid, now)
            .await
            .map_err(hw_repo_status)?;

        self.hw_deps
            .audit
            .write(HardwareTokenAuditEntry {
                log_id: Uuid::new_v4(),
                actor_uuid: Some(owner_uuid),
                actor_type: "PLAYER",
                target_token_id: Some(updated.token_id),
                target_owner_uuid: Some(updated.owner_uuid),
                op: "token.revoke",
                hardware_id_hash: updated.hardware_id_hash.clone(),
                reason: None,
                tick_millis: tick,
                request_id: None,
            })
            .await
            .map_err(hw_repo_status)?;

        Ok(Response::new(hw_token_to_response(&updated, "hw.revoked")))
    }

    // ── Authenticate (doc/18 §3.1) ──────────────────────────
    async fn authenticate(
        &self,
        request: Request<AuthenticateRequest>,
    ) -> Result<Response<AuthenticateResponse>, Status> {
        let AuthenticateRequest {
            player_uuid,
            player_username,
            hardware_id_hash,
        } = request.into_inner();
        let tick = (self.tick_millis)();

        // 1. Whitelist fast path (18 §1.1 + §3.1) — 内存 cache O(1).
        let whitelist_hit = self
            .whitelist
            .read()
            .map(|w| w.contains(player_uuid, &player_username))
            .unwrap_or(false);
        if whitelist_hit {
            self.hw_deps
                .audit
                .write(HardwareTokenAuditEntry {
                    log_id: Uuid::new_v4(),
                    actor_uuid: Some(player_uuid),
                    actor_type: "PLAYER",
                    target_token_id: None,
                    target_owner_uuid: Some(player_uuid),
                    op: "auth.whitelist_pass",
                    hardware_id_hash: Some(hardware_id_hash),
                    reason: None,
                    tick_millis: tick,
                    request_id: None,
                })
                .await
                .map_err(hw_repo_status)?;
            return Ok(Response::new(AuthenticateResponse {
                allowed: true,
                reason: "whitelisted".to_owned(),
                event_meta: EventMeta {
                    kind: "auth.allow".to_owned(),
                    payload_json: serde_json::json!({
                        "reason": "whitelisted",
                        "player_uuid": player_uuid,
                        "player_username": player_username,
                    })
                    .to_string(),
                },
            }));
        }

        // 2. Hot path: find a BOUND non-expired token whose
        //    hardware_id_hash matches (18 §3.1 step 2).
        let now = Utc::now();
        let match_ = self
            .hw_deps
            .repo
            .find_valid_token(player_uuid, &hardware_id_hash, now)
            .await
            .map_err(hw_repo_status)?;

        if let Some(token) = match_ {
            self.hw_deps
                .audit
                .write(HardwareTokenAuditEntry {
                    log_id: Uuid::new_v4(),
                    actor_uuid: Some(player_uuid),
                    actor_type: "PLAYER",
                    target_token_id: Some(token.token_id),
                    target_owner_uuid: Some(player_uuid),
                    op: "auth.hardware_pass",
                    hardware_id_hash: Some(hardware_id_hash),
                    reason: None,
                    tick_millis: tick,
                    request_id: None,
                })
                .await
                .map_err(hw_repo_status)?;
            return Ok(Response::new(AuthenticateResponse {
                allowed: true,
                reason: "hardware_token_valid".to_owned(),
                event_meta: EventMeta {
                    kind: "auth.allow".to_owned(),
                    payload_json: serde_json::json!({
                        "reason": "hardware_token_valid",
                        "token_id": token.token_id,
                    })
                    .to_string(),
                },
            }));
        }

        // 3. No match. Disambiguate the deny reason (18 §3.1
        //    steps 4-6).
        let reason = self
            .deny_reason(player_uuid, &hardware_id_hash, now)
            .await
            .unwrap_or_else(|_| "no_hardware_token".to_owned());

        self.hw_deps
            .audit
            .write(HardwareTokenAuditEntry {
                log_id: Uuid::new_v4(),
                actor_uuid: Some(player_uuid),
                actor_type: "PLAYER",
                target_token_id: None,
                target_owner_uuid: Some(player_uuid),
                op: "auth.deny",
                hardware_id_hash: Some(hardware_id_hash),
                reason: Some(reason.clone()),
                tick_millis: tick,
                request_id: None,
            })
            .await
            .map_err(hw_repo_status)?;

        Ok(Response::new(AuthenticateResponse {
            allowed: false,
            reason: reason.clone(),
            event_meta: EventMeta {
                kind: "auth.deny".to_owned(),
                payload_json: serde_json::json!({
                    "reason": reason,
                    "player_uuid": player_uuid,
                })
                .to_string(),
            },
        }))
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Auto-create the player's account row when it doesn't exist yet.
/// The Rust path owns the durable ledger, so the first transaction
/// from a brand-new player must result in a row — not the Java
/// `BankManager` default of "0 in memory".
async fn ensure_account(
    repo: &std::sync::Arc<dyn BankRepository>,
    owner: Uuid,
    tick: i64,
) -> Result<biocapital_bank::domain::BankAccount, Status> {
    match repo.get_account_by_owner(owner).await {
        Ok(a) => Ok(a),
        Err(BankRepoError::AccountNotFound { .. }) => {
            let account = biocapital_bank::domain::BankAccount {
                account_uuid: Uuid::new_v4(),
                owner_uuid: owner,
                balance: 0,
                max_balance: MAX_BALANCE,
                device_lock: None,
                created_tick: tick,
                updated_tick: tick,
            };
            repo.upsert_account(&account).await.map_err(repo_status)?;
            Ok(account)
        }
        Err(e) => Err(repo_status(e)),
    }
}

fn event_meta_for_tx(tx: &BankTransaction) -> EventMeta {
    EventMeta {
        kind: "bank.tx".to_owned(),
        payload_json: serde_json::json!({
            "tx_id": tx.tx_id,
            "account_uuid": tx.account_uuid,
            "op": tx.op.as_str(),
            "amount": tx.amount,
            "balance_after": tx.balance_after,
            "counterparty_uuid": tx.counterparty_uuid,
            "tick_millis": tx.tick_millis,
        })
        .to_string(),
    }
}

fn domain_to_status(e: biocapital_bank::domain::BankError) -> Status {
    use biocapital_bank::domain::BankError as B;
    match e {
        B::InsufficientFunds { have, want } => {
            Status::failed_precondition(format!("insufficient funds: have {have}, want {want}"))
        }
        B::ZeroBalance => Status::failed_precondition("balance is zero"),
        B::OverMax { max, have, add } => Status::failed_precondition(format!(
            "would exceed max balance ({max}): have {have}, add {add}"
        )),
        B::DeviceLocked { locked, presented } => Status::permission_denied(format!(
            "device lock mismatch: locked to {locked:?}, presented {presented:?}"
        )),
        B::NonPositiveAmount(n) => {
            Status::invalid_argument(format!("amount must be positive, got {n}"))
        }
    }
}

fn repo_status(e: BankRepoError) -> Status {
    use BankRepoError as R;
    match e {
        R::AccountNotFound { account_uuid } => {
            Status::not_found(format!("bank account {account_uuid} not found"))
        }
        R::BatchNotFound { batch_id } => {
            Status::not_found(format!("cat-grass batch {batch_id} not found"))
        }
        R::Sqlx(sqlx::Error::RowNotFound) => {
            Status::not_found("bank row not found")
        }
        R::Sqlx(e) => Status::internal(format!("postgres error: {e}")),
        R::Migrate(e) => Status::internal(format!("migration error: {e}")),
        R::InvalidUuid { column, value } => {
            Status::invalid_argument(format!("invalid UUID in {column}: {value}"))
        }
        R::InvalidBankOp { column, value } => {
            Status::invalid_argument(format!("invalid BankOp in {column}: {value}"))
        }
        R::InvalidSource { column, value } => {
            Status::invalid_argument(format!("invalid CatGrassSource in {column}: {value}"))
        }
    }
}

// ── 硬件 token 子系统 helpers (doc/18 §3.1 + §7) ────────────────────────────

/// Map a `HardwareTokenRepoError` to a `tonic::Status`. Mirrors
/// `repo_status` for the bank module.
fn hw_repo_status(e: HardwareTokenRepoError) -> Status {
    use HardwareTokenRepoError as R;
    match e {
        R::TokenNotFound { token_id, owner_uuid } => Status::not_found(format!(
            "hardware token {token_id} not found for owner {owner_uuid}"
        )),
        R::InvalidUuid { column, value } => {
            Status::invalid_argument(format!("invalid UUID in {column}: {value}"))
        }
        R::InvalidStatus { column, value } => {
            Status::invalid_argument(format!(
                "invalid HardwareTokenStatus in {column}: {value}"
            ))
        }
        R::Sqlx(sqlx::Error::RowNotFound) => {
            Status::not_found("hardware_token row not found")
        }
        R::Sqlx(e) => {
            // Domain-validation errors come through here as
            // `Protocol`. Surface them as `failed_precondition`
            // so the Java side can show a clean message; raw
            // SQL errors stay `internal`.
            let msg = e.to_string();
            if msg.contains("non-bindable state") {
                Status::failed_precondition(msg)
            } else {
                Status::internal(format!("postgres error: {e}"))
            }
        }
        R::Migrate(e) => Status::internal(format!("migration error: {e}")),
    }
}

/// Project a `HardwareToken` (domain) into the proto-shaped
/// `TokenResponse` (gRPC wire). The `last_used_tick` /
/// `created_at` are filled from the slot's `bound_at` /
/// `issued_at` (the dglab `dglab_tokens` analogue — a
/// hardware-token row is created on `RequestHardwareToken` and
/// "used" on `BindHardware`).
fn hw_token_to_response(t: &HardwareToken, kind: &str) -> TokenResponse {
    let created_tick = t.issued_at.timestamp_millis();
    let last_used_tick = t
        .bound_at
        .map(|b| b.timestamp_millis())
        .unwrap_or(created_tick);
    let enabled = matches!(t.status, HardwareTokenStatus::Bound);
    let token_id = t.token_id;
    let owner_uuid = t.owner_uuid;
    TokenResponse {
        token: t.token_id,
        owner_uuid: t.owner_uuid,
        created_tick,
        last_used_tick,
        enabled,
        event_meta: EventMeta {
            kind: kind.to_owned(),
            payload_json: serde_json::json!({
                "token_id": token_id,
                "owner_uuid": owner_uuid,
                "status": t.status.as_str(),
                "expires_at": t.expires_at.to_rfc3339(),
            })
            .to_string(),
        },
    }
}

impl BankGrpc {
    /// 18 §3.1 steps 4-6: disambiguate the deny reason after
    /// `find_valid_token` returns `None`. The decision tree:
    ///
    /// 1. If the player has **any** BOUND token (any hash) but
    ///    none match the incoming `hardware_id_hash` →
    ///    `"hardware_id_mismatch"`.
    /// 2. Else if the player has any token at all (including
    ///    REPLACED / REVOKED) and they're all past
    ///    `expires_at` → `"hardware_token_expired"`.
    /// 3. Else → `"no_hardware_token"`.
    ///
    /// The function is best-effort: a read failure falls
    /// through to `"no_hardware_token"`.
    async fn deny_reason(
        &self,
        player_uuid: Uuid,
        _incoming_hash: &str,
        now: chrono::DateTime<Utc>,
    ) -> Result<String, Status> {
        // Pull all rows (terminal + alive) so we can see
        // historical / REPLACED / REVOKED entries.
        let rows = self
            .hw_deps
            .repo
            .list_for_owner(player_uuid, true)
            .await
            .map_err(hw_repo_status)?;

        if rows.is_empty() {
            return Ok("no_hardware_token".to_owned());
        }

        // Step 1: any BOUND entry whose hash ≠ incoming
        // (already known to be the case from `find_valid_token`
        // returning None).
        let has_bound_mismatch = rows.iter().any(|t| {
            t.status == HardwareTokenStatus::Bound
                && t.hardware_id_hash.is_some()
        });
        if has_bound_mismatch {
            return Ok("hardware_id_mismatch".to_owned());
        }

        // Step 2: every active entry expired.
        let all_expired = rows
            .iter()
            .filter(|t| t.status == HardwareTokenStatus::Bound)
            .all(|t| t.is_expired_at(now));
        if all_expired && !rows.is_empty() {
            return Ok("hardware_token_expired".to_owned());
        }

        Ok("no_hardware_token".to_owned())
    }
}

// ── Tests (no live DB) ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    use async_trait::async_trait;

    use biocapital_bank::domain::{
        new_batch, BankAccount, CatGrassBatch, MAX_BALANCE,
    };
    use biocapital_pg::{BankAuditEntry, BankAuditWriter, BankRepository, BankServiceDeps};

    struct MemRepo {
        accounts: Mutex<HashMap<Uuid, BankAccount>>,
        owner_idx: Mutex<HashMap<Uuid, Uuid>>,
        tx: Mutex<Vec<BankTransaction>>,
        batches: Mutex<HashMap<Uuid, CatGrassBatch>>,
    }

    impl MemRepo {
        fn new() -> Self {
            Self {
                accounts: Mutex::new(HashMap::new()),
                owner_idx: Mutex::new(HashMap::new()),
                tx: Mutex::new(Vec::new()),
                batches: Mutex::new(HashMap::new()),
            }
        }
        fn touch(&self, account: BankAccount) {
            self.owner_idx
                .lock()
                .unwrap()
                .insert(account.owner_uuid, account.account_uuid);
            self.accounts
                .lock()
                .unwrap()
                .insert(account.account_uuid, account);
        }
    }

    #[async_trait]
    impl BankRepository for MemRepo {
        async fn get_account(&self, uuid: Uuid) -> Result<BankAccount, BankRepoError> {
            self.accounts
                .lock()
                .unwrap()
                .get(&uuid)
                .cloned()
                .ok_or(BankRepoError::AccountNotFound { account_uuid: uuid })
        }
        async fn get_account_by_owner(
            &self,
            owner: Uuid,
        ) -> Result<BankAccount, BankRepoError> {
            let idx = self.owner_idx.lock().unwrap();
            let acc_id = idx
                .get(&owner)
                .copied()
                .ok_or(BankRepoError::AccountNotFound { account_uuid: owner })?;
            drop(idx);
            self.accounts
                .lock()
                .unwrap()
                .get(&acc_id)
                .cloned()
                .ok_or(BankRepoError::AccountNotFound { account_uuid: acc_id })
        }
        async fn upsert_account(&self, account: &BankAccount) -> Result<(), BankRepoError> {
            self.touch(account.clone());
            Ok(())
        }
        async fn atomic_deposit(
            &self,
            uuid: Uuid,
            amount: i64,
            _batch_id: Uuid,
            request_id: Uuid,
            tick_millis: i64,
        ) -> Result<BankTransaction, BankRepoError> {
            // Idempotency
            if let Some(t) = self
                .tx
                .lock()
                .unwrap()
                .iter()
                .find(|t| t.request_id == request_id)
                .cloned()
            {
                return Ok(t);
            }
            let mut accounts = self.accounts.lock().unwrap();
            let acc = accounts
                .get_mut(&uuid)
                .ok_or(BankRepoError::AccountNotFound { account_uuid: uuid })?;
            let accepted = amount.min(acc.max_balance - acc.balance).max(0);
            acc.balance += accepted;
            let tx = BankTransaction {
                tx_id: Uuid::new_v4(),
                account_uuid: uuid,
                op: BankOp::Deposit,
                amount: accepted,
                balance_after: acc.balance,
                counterparty_uuid: None,
                counterparty_name: None,
                tick_millis,
                request_id,
            };
            self.tx.lock().unwrap().push(tx.clone());
            Ok(tx)
        }
        async fn atomic_withdraw(
            &self,
            uuid: Uuid,
            amount: i64,
            request_id: Uuid,
            tick_millis: i64,
        ) -> Result<BankTransaction, BankRepoError> {
            if let Some(t) = self
                .tx
                .lock()
                .unwrap()
                .iter()
                .find(|t| t.request_id == request_id)
                .cloned()
            {
                return Ok(t);
            }
            let mut accounts = self.accounts.lock().unwrap();
            let acc = accounts
                .get_mut(&uuid)
                .ok_or(BankRepoError::AccountNotFound { account_uuid: uuid })?;
            let taken = amount.min(acc.balance).max(0);
            acc.balance -= taken;
            let tx = BankTransaction {
                tx_id: Uuid::new_v4(),
                account_uuid: uuid,
                op: BankOp::Withdraw,
                amount: taken,
                balance_after: acc.balance,
                counterparty_uuid: None,
                counterparty_name: None,
                tick_millis,
                request_id,
            };
            self.tx.lock().unwrap().push(tx.clone());
            Ok(tx)
        }
        async fn atomic_transfer(
            &self,
            from: Uuid,
            to: Uuid,
            amount: i64,
            counterparty_name: Option<String>,
            request_id: Uuid,
            tick_millis: i64,
        ) -> Result<(BankTransaction, BankTransaction), BankRepoError> {
            if let Some(t) = self
                .tx
                .lock()
                .unwrap()
                .iter()
                .find(|t| t.request_id == request_id)
                .cloned()
            {
                let counter = self
                    .tx
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|t| t.request_id == request_id && t.account_uuid == to)
                    .cloned()
                    .ok_or_else(|| {
                        BankRepoError::Sqlx(sqlx::Error::Protocol("counter missing".into()))
                    })?;
                return Ok((t, counter));
            }
            let mut accounts = self.accounts.lock().unwrap();
            // Compute taken/credited from immutable reads of `accounts`
            // (so the two &mut borrows below do not overlap), then mutate.
            // We use a helper closure so the immutable borrows end before
            // the mutable borrows begin (NLL drops them at end of scope).
            let (taken, credited) = {
                let f = accounts
                    .get(&from)
                    .ok_or(BankRepoError::AccountNotFound { account_uuid: from })?;
                let t = accounts
                    .get(&to)
                    .ok_or(BankRepoError::AccountNotFound { account_uuid: to })?;
                let taken = amount.min(f.balance).max(0);
                let headroom = t.max_balance - t.balance;
                (taken, taken.min(headroom).max(0))
            };
            // The block above ends here — `f` / `t` immutable borrows are
            // released. Now we can take the two &mut borrows sequentially
            // and read back the new balances for the audit transactions.
            let f_after = {
                let a = accounts.get_mut(&from).expect("from account exists");
                a.balance -= taken;
                a.balance
            };
            let t_after = {
                let a = accounts.get_mut(&to).expect("to account exists");
                a.balance += credited;
                a.balance
            };
            let out = BankTransaction {
                tx_id: Uuid::new_v4(),
                account_uuid: from,
                op: BankOp::TransferOut,
                amount: taken,
                balance_after: f_after,
                counterparty_uuid: Some(to),
                counterparty_name: counterparty_name.clone(),
                tick_millis,
                request_id,
            };
            let inn = BankTransaction {
                tx_id: Uuid::new_v4(),
                account_uuid: to,
                op: BankOp::TransferIn,
                amount: credited,
                balance_after: t_after,
                counterparty_uuid: Some(from),
                counterparty_name: None,
                tick_millis,
                request_id,
            };
            self.tx.lock().unwrap().push(out.clone());
            self.tx.lock().unwrap().push(inn.clone());
            Ok((out, inn))
        }
        async fn get_history(
            &self,
            uuid: Uuid,
            limit: i64,
        ) -> Result<Vec<BankTransaction>, BankRepoError> {
            let g = self.tx.lock().unwrap();
            let mut out: Vec<_> = g
                .iter()
                .filter(|t| t.account_uuid == uuid)
                .cloned()
                .collect();
            out.sort_by_key(|t| std::cmp::Reverse(t.tick_millis));
            out.truncate(limit as usize);
            Ok(out)
        }
        async fn lock_device(
            &self,
            uuid: Uuid,
            device_id: String,
        ) -> Result<BankAccount, BankRepoError> {
            let mut g = self.accounts.lock().unwrap();
            let a = g
                .get_mut(&uuid)
                .ok_or(BankRepoError::AccountNotFound { account_uuid: uuid })?;
            a.device_lock = Some(device_id);
            Ok(a.clone())
        }
        async fn unlock_device(
            &self,
            uuid: Uuid,
            _invite_code: Uuid,
        ) -> Result<BankAccount, BankRepoError> {
            let mut g = self.accounts.lock().unwrap();
            let a = g
                .get_mut(&uuid)
                .ok_or(BankRepoError::AccountNotFound { account_uuid: uuid })?;
            a.device_lock = None;
            Ok(a.clone())
        }
        async fn create_batch(
            &self,
            batch: &CatGrassBatch,
        ) -> Result<(), BankRepoError> {
            self.batches
                .lock()
                .unwrap()
                .insert(batch.batch_id, batch.clone());
            Ok(())
        }
        async fn consume_batch(
            &self,
            batch_id: Uuid,
            amount: i64,
        ) -> Result<CatGrassBatch, BankRepoError> {
            let mut g = self.batches.lock().unwrap();
            let b = g
                .get_mut(&batch_id)
                .ok_or(BankRepoError::BatchNotFound { batch_id })?;
            let taken = amount.min(b.remaining_amount).max(0);
            b.remaining_amount -= taken;
            Ok(b.clone())
        }
        async fn list_batches_by_holder(
            &self,
            holder: Uuid,
        ) -> Result<Vec<CatGrassBatch>, BankRepoError> {
            Ok(self
                .batches
                .lock()
                .unwrap()
                .values()
                .filter(|b| b.current_holder_uuid == Some(holder))
                .cloned()
                .collect())
        }
    }

    struct MemAudit {
        rows: Mutex<Vec<BankAuditEntry>>,
    }
    impl MemAudit {
        fn new() -> Self {
            Self { rows: Mutex::new(Vec::new()) }
        }
    }
    #[async_trait]
    impl BankAuditWriter for MemAudit {
        async fn write(&self, entry: BankAuditEntry) -> Result<(), BankRepoError> {
            self.rows.lock().unwrap().push(entry);
            Ok(())
        }
    }

    fn make_service() -> (BankGrpc, Arc<MemRepo>, Arc<MemAudit>) {
        let repo = Arc::new(MemRepo::new());
        let audit = Arc::new(MemAudit::new());
        let deps = BankServiceDeps::new(
            repo.clone() as Arc<dyn BankRepository>,
            audit.clone() as Arc<dyn BankAuditWriter>,
        );
        (BankGrpc::new(deps), repo, audit)
    }

    #[tokio::test]
    async fn get_balance_returns_zero_for_unknown_player() {
        let (svc, _repo, _audit) = make_service();
        let resp = svc
            .get_balance(Request::new(AccountRequest {
                player: PlayerIdentifier { player_uuid: Uuid::new_v4() },
                device_id: None,
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(resp.balance, 0);
        assert_eq!(resp.max_balance, MAX_BALANCE);
        assert_eq!(resp.event_meta.kind, "");
    }

    #[tokio::test]
    async fn deposit_then_withdraw_clamps_and_audits() {
        let (svc, _repo, audit) = make_service();
        let player = Uuid::new_v4();
        // Deposit 50.
        let _ = svc
            .deposit(Request::new(DepositRequest {
                player: PlayerIdentifier { player_uuid: player },
                amount: 50,
                batch_id: Uuid::new_v4(),
                request_id: Uuid::new_v4(),
            }))
            .await
            .unwrap()
            .into_inner();
        // Withdraw 100 (overdraw → 0).
        let resp = svc
            .withdraw(Request::new(WithdrawRequest {
                player: PlayerIdentifier { player_uuid: player },
                amount: 100,
                request_id: Uuid::new_v4(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(resp.balance, 0);
        let rows = audit.rows.lock().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].op, "bank.deposit");
        assert_eq!(rows[1].op, "bank.withdraw");
    }

    #[tokio::test]
    async fn transfer_rejects_zero_balance() {
        let (svc, _repo, _audit) = make_service();
        let from = Uuid::new_v4();
        let to = Uuid::new_v4();
        let status = svc
            .transfer(Request::new(TransferRequest {
                from: PlayerIdentifier { player_uuid: from },
                to: PlayerIdentifier { player_uuid: to },
                amount: 1,
                memo: None,
                request_id: Uuid::new_v4(),
            }))
            .await
            .err()
            .unwrap();
        assert_eq!(status.code(), tonic::Code::FailedPrecondition);
    }

    #[tokio::test]
    async fn transfer_moves_funds() {
        let (svc, _repo, audit) = make_service();
        let from = Uuid::new_v4();
        let to = Uuid::new_v4();
        let _ = svc
            .deposit(Request::new(DepositRequest {
                player: PlayerIdentifier { player_uuid: from },
                amount: 100,
                batch_id: Uuid::new_v4(),
                request_id: Uuid::new_v4(),
            }))
            .await
            .unwrap();
        let _ = svc
            .transfer(Request::new(TransferRequest {
                from: PlayerIdentifier { player_uuid: from },
                to: PlayerIdentifier { player_uuid: to },
                amount: 40,
                memo: Some("hi".into()),
                request_id: Uuid::new_v4(),
            }))
            .await
            .unwrap();
        let from_balance = svc
            .get_balance(Request::new(AccountRequest {
                player: PlayerIdentifier { player_uuid: from },
                device_id: None,
            }))
            .await
            .unwrap()
            .into_inner()
            .balance;
        let to_balance = svc
            .get_balance(Request::new(AccountRequest {
                player: PlayerIdentifier { player_uuid: to },
                device_id: None,
            }))
            .await
            .unwrap()
            .into_inner()
            .balance;
        assert_eq!(from_balance, 60);
        assert_eq!(to_balance, 40);
        let rows = audit.rows.lock().unwrap();
        assert!(rows.iter().any(|r| r.op == "bank.transfer"));
    }

    #[tokio::test]
    async fn lock_device_marks_locked() {
        let (svc, _repo, _audit) = make_service();
        let player = Uuid::new_v4();
        let _ = svc
            .deposit(Request::new(DepositRequest {
                player: PlayerIdentifier { player_uuid: player },
                amount: 1,
                batch_id: Uuid::new_v4(),
                request_id: Uuid::new_v4(),
            }))
            .await
            .unwrap();
        let resp = svc
            .lock_device(Request::new(LockDeviceRequest {
                player: PlayerIdentifier { player_uuid: player },
                device_id: "dev-a".into(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(resp.success);
        assert_eq!(resp.current_device_id.as_deref(), Some("dev-a"));
        assert_eq!(resp.event_meta.kind, "card.lock");
    }

    #[tokio::test]
    async fn get_history_returns_recent_entries() {
        let (svc, _repo, _audit) = make_service();
        let player = Uuid::new_v4();
        for _ in 0..3 {
            let _ = svc
                .deposit(Request::new(DepositRequest {
                    player: PlayerIdentifier { player_uuid: player },
                    amount: 5,
                    batch_id: Uuid::new_v4(),
                    request_id: Uuid::new_v4(),
                }))
                .await
                .unwrap();
        }
        let resp = svc
            .get_history(Request::new(HistoryRequest {
                player: PlayerIdentifier { player_uuid: player },
                limit: 16,
                before_tick_millis: None,
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(resp.entries.len(), 3);
    }

    #[tokio::test]
    async fn batch_consume_clamps() {
        let (_svc, _repo, _audit) = make_service();
        let mut b = new_batch(10, biocapital_bank::domain::CatGrassSource::AtmDeposit, None, 0);
        let taken = b.consume(50).unwrap();
        assert_eq!(taken, 10);
        assert!(b.is_exhausted());
    }
}
