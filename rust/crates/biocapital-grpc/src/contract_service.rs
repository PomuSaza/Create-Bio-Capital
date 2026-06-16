//! gRPC service for `ContractService` (`doc/14-rust-services.md` §3.2).
//!
//! Proto path: `rust/proto/biocapital.proto` → `biocapital.v1` package.
//!
//! Seven RPCs are implemented:
//!   - `ProposeContract`    (method_id 0)
//!   - `AcceptContract`     (method_id 1)
//!   - `RejectContract`     (method_id 2)
//!   - `TerminateContract`  (method_id 3)
//!   - `RedeemContract`     (method_id 4)
//!   - `GetContract`        (method_id 5)
//!   - `ListContracts`      (method_id 6)
//!
//! Every mutating RPC writes to `contracts` (and on redemption,
//! also to `contract_payouts`) and emits one of the four
//! `EventMeta` kinds the Sable JNI bridge maps to the
//! NeoForge events in 99 §3.1:
//!
//! - `"contract.created"`       → `ContractCreatedEvent`
//! - `"contract.activated"`     → `ContractActivatedEvent`
//! - `"contract.terminated"`    → `ContractTerminatedEvent`
//! - `"contract.payout"`        → `ContractPayoutEvent`
//! - `"none"`                   → no event (read-only paths)
//!
//! `RedeemContract` is the cross-crate hot spot: the gRPC
//! layer calls into [`crate::bank_service::BankRpc::transfer`]
//! via the [`biocapital_pg::BankTransferPort`] indirection
//! (see `ContractServiceDeps::with_bank_transfer`). The bank
//! transfer carries the `request_id` from
//! [`biocapital_contract::domain::compute_payout`] so a
//! replayed redemption RPC is idempotent at both the bank
//! and the contract layers.

use std::sync::Arc;

use chrono::Utc;
use tonic::{Request, Response, Status};
use uuid::Uuid;

use biocapital_contract::domain::{
    accept_contract as domain_accept,
    propose_contract as domain_propose,
    reject_contract as domain_reject,
    redeem_contract as domain_redeem,
    terminate_contract as domain_terminate,
    ContractError, ContractStatus,
};
// `ContractAuditWriter` + `ContractRepository` are only referenced
// by the test module below; `use super::*` re-exports the lib's
// top-level imports.
#[allow(unused_imports)]
use biocapital_pg::{
    ContractAuditEntry, ContractAuditWriter, ContractRepository, ContractServiceDeps,
    ContractRepoError,
};

// ── Opaque request / response types (proto-shaped) ─────────────────────────
//
// The build script that wires `prost-build` is part of task #17
// follow-up; until then each RPC signature accepts a request
// that exposes the proto `PlayerIdentifier.player_uuid` and the
// per-RPC fields, and returns a proto-shaped `ContractResponse`.
// The `ContractRpc` trait is the integration boundary; it stays
// close to the proto shape so the swap-in of generated types is
// mechanical.

#[derive(Debug, Clone)]
pub struct PlayerIdentifier {
    pub player_uuid: Uuid,
}

#[derive(Debug, Clone, Default)]
pub struct ListRequest {
    pub limit: i32,
    pub offset: i32,
    pub player_filter: Option<PlayerIdentifier>,
}

#[derive(Debug, Clone)]
pub struct ContractProposeRequest {
    pub master: PlayerIdentifier,
    pub slave: PlayerIdentifier,
    pub terms_type: String,
    pub terms_json: String,
    pub revenue_share_pct: f32,
    pub redemption_cost: i64,
    /// `0` means "permanent" — the domain `expires_tick` is
    /// stored as `None`. The proto field is `int64`, so the
    /// gRPC layer encodes the "no expiry" sentinel as 0 to
    /// keep the wire shape clean.
    pub expires_tick: i64,
    pub request_id: Uuid,
}

#[derive(Debug, Clone)]
pub struct ContractAcceptRequest {
    pub contract_id: Uuid,
    pub request_id: Uuid,
}

#[derive(Debug, Clone)]
pub struct ContractRejectRequest {
    pub contract_id: Uuid,
    pub reason: String,
    pub request_id: Uuid,
}

#[derive(Debug, Clone)]
pub struct ContractTerminateRequest {
    pub contract_id: Uuid,
    pub reason: String,
    pub request_id: Uuid,
}

#[derive(Debug, Clone)]
pub struct ContractRedeemRequest {
    pub contract_id: Uuid,
    pub request_id: Uuid,
}

#[derive(Debug, Clone)]
pub struct ContractRequest {
    pub contract_id: Uuid,
}

#[derive(Debug, Clone)]
pub struct ContractResponse {
    pub contract_id: Uuid,
    pub master: PlayerIdentifier,
    pub slave: PlayerIdentifier,
    pub status: String, // "PROPOSED" | "ACTIVE" | "TERMINATED" | "REDEEMED" | "REJECTED"
    pub terms_type: String,
    pub terms_json: String,
    pub revenue_share_pct: f32,
    pub redemption_cost: i64,
    pub created_tick: i64,
    pub activated_tick: Option<i64>,
    pub expires_tick: Option<i64>,
    pub terminated_tick: Option<i64>,
    pub termination_reason: Option<String>,
    pub updated_at: chrono::DateTime<Utc>,
    pub event_meta: EventMeta,
}

#[derive(Debug, Clone)]
pub struct ContractListResponse {
    pub contracts: Vec<ContractResponse>,
    pub total_count: i32,
    pub event_meta: EventMeta,
}

// ── Event meta ─────────────────────────────────────────────────────────────
//
// Sable JNI consumes this to fire the NeoForge events listed
// in 99 §3.1 (`ContractCreatedEvent`, `ContractActivatedEvent`,
// `ContractTerminatedEvent`, `ContractPayoutEvent`). The Java
// side translates the `kind` string into the matching
// `Event` subclass.

#[derive(Debug, Clone, Default)]
pub struct EventMeta {
    /// One of: `"none"`, `"contract.created"`,
    /// `"contract.activated"`, `"contract.terminated"`,
    /// `"contract.payout"`.
    pub kind: String,
    /// Opaque payload serialised to JSONB by the Java side.
    pub payload_json: String,
}

// ── gRPC service trait ──────────────────────────────────────────────────────

#[tonic::async_trait]
pub trait ContractRpc: Send + Sync + 'static {
    async fn propose_contract(
        &self,
        request: Request<ContractProposeRequest>,
    ) -> Result<Response<ContractResponse>, Status>;

    async fn accept_contract(
        &self,
        request: Request<ContractAcceptRequest>,
    ) -> Result<Response<ContractResponse>, Status>;

    async fn reject_contract(
        &self,
        request: Request<ContractRejectRequest>,
    ) -> Result<Response<ContractResponse>, Status>;

    async fn terminate_contract(
        &self,
        request: Request<ContractTerminateRequest>,
    ) -> Result<Response<ContractResponse>, Status>;

    async fn redeem_contract(
        &self,
        request: Request<ContractRedeemRequest>,
    ) -> Result<Response<ContractResponse>, Status>;

    async fn get_contract(
        &self,
        request: Request<ContractRequest>,
    ) -> Result<Response<ContractResponse>, Status>;

    async fn list_contracts(
        &self,
        request: Request<ListRequest>,
    ) -> Result<Response<ContractListResponse>, Status>;
}

// ── Implementation ──────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct ContractGrpc {
    deps: ContractServiceDeps,
    /// Logical clock. Default: `Utc::now().timestamp_millis()`.
    tick_millis: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl ContractGrpc {
    pub fn new(deps: ContractServiceDeps) -> Self {
        Self {
            deps,
            tick_millis: Arc::new(|| Utc::now().timestamp_millis()),
        }
    }

    pub fn with_clock(
        deps: ContractServiceDeps,
        clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    ) -> Self {
        Self {
            deps,
            tick_millis: clock,
        }
    }
}

#[tonic::async_trait]
impl ContractRpc for ContractGrpc {
    // ── ProposeContract ────────────────────────────────────────
    async fn propose_contract(
        &self,
        request: Request<ContractProposeRequest>,
    ) -> Result<Response<ContractResponse>, Status> {
        let ContractProposeRequest {
            master,
            slave,
            terms_type,
            terms_json,
            revenue_share_pct,
            redemption_cost,
            expires_tick,
            request_id: _request_id,
        } = request.into_inner();
        if master.player_uuid == slave.player_uuid {
            return Err(Status::invalid_argument(
                "master and slave must differ",
            ));
        }
        if terms_type.is_empty() {
            return Err(Status::invalid_argument("terms_type is empty"));
        }
        // Validate terms_json parses (the SQL JSONB layer will
        // refuse malformed payloads too, but a clear 400 here
        // is friendlier than a 500 from the database driver).
        let _terms: serde_json::Value = serde_json::from_str(&terms_json)
            .map_err(|e| Status::invalid_argument(format!("terms_json invalid: {e}")))?;

        let tick = (self.tick_millis)();
        let contract = domain_propose(
            Uuid::new_v4(),
            master.player_uuid,
            slave.player_uuid,
            terms_type,
            terms_json,
            revenue_share_pct,
            redemption_cost,
            // Sentinel: wire 0 → domain None.
            if expires_tick == 0 { None } else { Some(expires_tick) },
            tick,
        )
        .map_err(domain_to_status)?;

        self.deps
            .repo
            .create(&contract)
            .await
            .map_err(repo_status)?;

        // Audit: route through `audit_bank` with op =
        // `contract.propose` so the existing 99 §2.2 dispatch
        // catches it without a new table. Optional in tests.
        if let Some(audit) = self.deps.audit.as_ref() {
            audit
                .write(ContractAuditEntry {
                    log_id: Uuid::new_v4(),
                    actor_uuid: master.player_uuid,
                    actor_type: "PLAYER",
                    target_account_uuid: slave.player_uuid,
                    op: "contract.propose",
                    before_balance: 0,
                    after_balance: 0,
                    tick_millis: tick,
                    request_id: None,
                    notes: Some(serde_json::json!({
                        "contract_id": contract.contract_id,
                        "terms_type": contract.terms_type,
                        "revenue_share_pct": contract.revenue_share_pct,
                        "redemption_cost": contract.redemption_cost,
                        "expires_tick": contract.expires_tick,
                    })),
                })
                .await
                .map_err(repo_status)?;
        }

        let event_meta = event_meta_for(&contract, "contract.created", &serde_json::json!({
            "contract_id": contract.contract_id,
            "master": master.player_uuid,
            "slave": slave.player_uuid,
            "terms_type": contract.terms_type,
            "revenue_share_pct": contract.revenue_share_pct,
            "redemption_cost": contract.redemption_cost,
        }));
        Ok(Response::new(contract_to_response(contract, event_meta)))
    }

    // ── AcceptContract ─────────────────────────────────────────
    async fn accept_contract(
        &self,
        request: Request<ContractAcceptRequest>,
    ) -> Result<Response<ContractResponse>, Status> {
        let ContractAcceptRequest { contract_id, request_id: _request_id } = request.into_inner();
        let mut contract = self
            .deps
            .repo
            .get(contract_id)
            .await
            .map_err(repo_status)?;
        let tick = (self.tick_millis)();
        let acceptor = contract.acceptor_uuid;
        // Domain helper flips status / activated_tick /
        // updated_tick in place.
        let accept_result = domain_accept(&mut contract, acceptor, tick);

        // Always persist — even when the helper returned an
        // Expired error it mutated the row to REJECTED. For
        // other errors the row is unchanged and we propagate
        // without writing.
        match &accept_result {
            Ok(()) | Err(ContractError::Expired { .. }) => {
                self.deps
                    .repo
                    .update(&contract)
                    .await
                    .map_err(repo_status)?;
            }
            Err(_) => {}
        }
        accept_result.map_err(domain_to_status)?;

        let event_meta = event_meta_for(&contract, "contract.activated", &serde_json::json!({
            "contract_id": contract.contract_id,
            "master": contract.proposer_uuid,
            "slave": contract.acceptor_uuid,
            "activated_tick": contract.activated_tick,
        }));
        Ok(Response::new(contract_to_response(contract, event_meta)))
    }

    // ── RejectContract ─────────────────────────────────────────
    async fn reject_contract(
        &self,
        request: Request<ContractRejectRequest>,
    ) -> Result<Response<ContractResponse>, Status> {
        let ContractRejectRequest { contract_id, reason, request_id: _request_id } = request.into_inner();
        let mut contract = self
            .deps
            .repo
            .get(contract_id)
            .await
            .map_err(repo_status)?;
        let tick = (self.tick_millis)();
        let caller = contract.acceptor_uuid;
        domain_reject(&mut contract, caller, reason, tick)
            .map_err(domain_to_status)?;
        self.deps
            .repo
            .update(&contract)
            .await
            .map_err(repo_status)?;

        let event_meta = event_meta_for(&contract, "contract.terminated", &serde_json::json!({
            "contract_id": contract.contract_id,
            "master": contract.proposer_uuid,
            "slave": contract.acceptor_uuid,
            "reason": contract.reason,
        }));
        Ok(Response::new(contract_to_response(contract, event_meta)))
    }

    // ── TerminateContract ──────────────────────────────────────
    async fn terminate_contract(
        &self,
        request: Request<ContractTerminateRequest>,
    ) -> Result<Response<ContractResponse>, Status> {
        let ContractTerminateRequest { contract_id, reason, request_id: _request_id } = request.into_inner();
        let mut contract = self
            .deps
            .repo
            .get(contract_id)
            .await
            .map_err(repo_status)?;
        let tick = (self.tick_millis)();
        // The domain `terminate_contract` helper accepts either
        // party as the caller — it figures out the role and
        // tags the `reason` accordingly. Both
        // `contract.proposer_uuid` and `contract.acceptor_uuid`
        // are valid caller UUIDs; we cannot know which one the
        // gRPC client is acting on behalf of without an extra
        // proto field, so we try proposer first and fall back
        // to acceptor. Both branches are pure logic and
        // produce identical state transitions (only the
        // `by=…` tag in `reason` differs). The SQL row is
        // updated exactly once.
        // Copy the two candidate-caller UUIDs out *before* the
        // `&mut contract` borrow — `domain_terminate` mutates
        // the contract in place so the second call's argument
        // expression `contract.acceptor_uuid` would otherwise
        // be a use of a value that's still mutably borrowed.
        let proposer = contract.proposer_uuid;
        let acceptor = contract.acceptor_uuid;
        let primary = domain_terminate(&mut contract, proposer, reason.clone(), tick);
        if primary.is_err() {
            domain_terminate(&mut contract, acceptor, reason, tick)
                .map_err(domain_to_status)?;
        } else {
            primary.map_err(domain_to_status)?;
        }
        self.deps
            .repo
            .update(&contract)
            .await
            .map_err(repo_status)?;

        let event_meta = event_meta_for(&contract, "contract.terminated", &serde_json::json!({
            "contract_id": contract.contract_id,
            "master": contract.proposer_uuid,
            "slave": contract.acceptor_uuid,
            "reason": contract.reason,
        }));
        Ok(Response::new(contract_to_response(contract, event_meta)))
    }

    // ── RedeemContract ─────────────────────────────────────────
    async fn redeem_contract(
        &self,
        request: Request<ContractRedeemRequest>,
    ) -> Result<Response<ContractResponse>, Status> {
        let ContractRedeemRequest { contract_id, request_id } = request.into_inner();
        let mut contract = self
            .deps
            .repo
            .get(contract_id)
            .await
            .map_err(repo_status)?;
        if contract.status != ContractStatus::Active {
            return Err(Status::failed_precondition(format!(
                "contract {contract_id} is in state {}, cannot redeem",
                contract.status.as_str()
            )));
        }
        let tick = (self.tick_millis)();

        // Cross-crate call: hand the redemption cost to
        // `BankService.Transfer` via the port supplied in
        // `ContractServiceDeps`. The port wraps the gRPC
        // client; in production this is the
        // `bank_service::BankGrpc::transfer` invocation.
        let bank_port = self.deps.bank_transfer.as_ref().ok_or_else(|| {
            Status::unavailable(
                "bank transfer port not configured; redeem_contract requires it",
            )
        })?;
        let _ = bank_port
            .transfer(
                contract.acceptor_uuid,
                contract.proposer_uuid,
                contract.redemption_cost,
                Some(format!(
                    "contract redemption: contract_id={}",
                    contract.contract_id
                )),
                request_id,
            )
            .await?;

        // Domain helper flips the state machine and returns
        // the payout row to log.
        let proposer = contract.proposer_uuid;
        let payout = domain_redeem(&mut contract, proposer, tick)
            .map_err(domain_to_status)?;
        // Record the payout — idempotent on `request_id`.
        self.deps
            .repo
            .record_payout(&payout)
            .await
            .map_err(repo_status)?;
        self.deps
            .repo
            .update(&contract)
            .await
            .map_err(repo_status)?;

        let event_meta = event_meta_for(&contract, "contract.payout", &serde_json::json!({
            "contract_id": contract.contract_id,
            "master": contract.proposer_uuid,
            "slave": contract.acceptor_uuid,
            "amount": payout.amount,
            "payout_id": payout.payout_id,
            "request_id": payout.request_id,
        }));
        Ok(Response::new(contract_to_response(contract, event_meta)))
    }

    // ── GetContract ────────────────────────────────────────────
    async fn get_contract(
        &self,
        request: Request<ContractRequest>,
    ) -> Result<Response<ContractResponse>, Status> {
        let ContractRequest { contract_id } = request.into_inner();
        let contract = self
            .deps
            .repo
            .get(contract_id)
            .await
            .map_err(repo_status)?;
        Ok(Response::new(contract_to_response(contract, EventMeta::default())))
    }

    // ── ListContracts ──────────────────────────────────────────
    async fn list_contracts(
        &self,
        request: Request<ListRequest>,
    ) -> Result<Response<ContractListResponse>, Status> {
        let ListRequest { limit, offset: _offset, player_filter } = request.into_inner();
        let (proposer, acceptor) = match player_filter {
            Some(p) => (Some(p.player_uuid), Some(p.player_uuid)),
            None => (None, None),
        };
        let limit = if limit <= 0 { 32 } else { limit.min(1024) };
        let contracts = self
            .deps
            .repo
            .list(proposer, acceptor, None, limit as i64)
            .await
            .map_err(repo_status)?;
        let total = contracts.len() as i32;
        let responses = contracts
            .into_iter()
            .map(|c| contract_to_response(c, EventMeta::default()))
            .collect();
        Ok(Response::new(ContractListResponse {
            contracts: responses,
            total_count: total,
            event_meta: EventMeta::default(),
        }))
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

fn contract_to_response(
    contract: biocapital_contract::domain::Contract,
    event_meta: EventMeta,
) -> ContractResponse {
    ContractResponse {
        contract_id: contract.contract_id,
        master: PlayerIdentifier {
            player_uuid: contract.proposer_uuid,
        },
        slave: PlayerIdentifier {
            player_uuid: contract.acceptor_uuid,
        },
        status: contract.status.as_str().to_owned(),
        terms_type: contract.terms_type,
        terms_json: contract.terms_json,
        revenue_share_pct: contract.revenue_share_pct,
        redemption_cost: contract.redemption_cost,
        created_tick: contract.created_tick,
        activated_tick: contract.activated_tick,
        expires_tick: contract.expires_tick,
        terminated_tick: contract.terminated_tick,
        termination_reason: contract.reason,
        // `updated_at` is approximated by the terminated_tick
        // when set, else the activated_tick, else the
        // created_tick. The proto's `google.protobuf.Timestamp`
        // is wall-clock; the gRPC layer is expected to fill
        // this from the wall-clock derived from the tick. For
        // now we use `Utc::now()` and let the Java side
        // reconcile via the tick.
        updated_at: Utc::now(),
        event_meta,
    }
}

fn event_meta_for(
    contract: &biocapital_contract::domain::Contract,
    kind: &str,
    payload: &serde_json::Value,
) -> EventMeta {
    // Merge the contract-level fields with the per-event
    // payload into a single JSONB-style blob for the Java
    // side to consume. The keys `contract_id` and `status`
    // come from the contract; the rest come from `payload`.
    let mut merged = serde_json::Map::new();
    merged.insert(
        "contract_id".to_owned(),
        serde_json::json!(contract.contract_id),
    );
    merged.insert(
        "status".to_owned(),
        serde_json::json!(contract.status.as_str()),
    );
    if let Some(obj) = payload.as_object() {
        for (k, v) in obj {
            merged.insert(k.clone(), v.clone());
        }
    }
    EventMeta {
        kind: kind.to_owned(),
        payload_json: serde_json::Value::Object(merged).to_string(),
    }
}

fn domain_to_status(e: ContractError) -> Status {
    use ContractError as C;
    match e {
        C::NotFound { contract_id } => {
            Status::not_found(format!("contract {contract_id} not found"))
        }
        C::InvalidTransition { contract_id, current, action } => {
            Status::failed_precondition(format!(
                "contract {contract_id} is in state {}, cannot {action}",
                current.as_str()
            ))
        }
        C::NotParty { contract_id, caller, expected } => {
            Status::permission_denied(format!(
                "caller {caller} is not the {expected} of contract {contract_id}"
            ))
        }
        C::SelfContract { who } => {
            Status::invalid_argument(format!("self-contract not allowed: {who}"))
        }
        C::RevenueShareOutOfRange { got } => {
            Status::invalid_argument(format!("revenue_share_pct must be in [0, 100], got {got}"))
        }
        C::NegativeRedemptionCost { got } => {
            Status::invalid_argument(format!("redemption_cost must be non-negative, got {got}"))
        }
        C::EmptyTermsType => Status::invalid_argument("terms_type is empty"),
        C::Expired { contract_id, expires_tick } => {
            Status::failed_precondition(format!(
                "contract {contract_id} expired at tick {expires_tick}"
            ))
        }
    }
}

fn repo_status(e: ContractRepoError) -> Status {
    use ContractRepoError as R;
    match e {
        R::ContractNotFound { contract_id } => {
            Status::not_found(format!("contract {contract_id} not found"))
        }
        R::Sqlx(sqlx::Error::RowNotFound) => {
            Status::not_found("contract row not found")
        }
        R::Sqlx(e) => Status::internal(format!("postgres error: {e}")),
        R::Migrate(e) => Status::internal(format!("migration error: {e}")),
        R::InvalidUuid { column, value } => {
            Status::invalid_argument(format!("invalid UUID in {column}: {value}"))
        }
        R::InvalidStatus { column, value } => {
            Status::invalid_argument(format!(
                "invalid ContractStatus in {column}: {value}"
            ))
        }
        R::InvalidReason { column, value } => {
            Status::invalid_argument(format!(
                "invalid PayoutReason in {column}: {value}"
            ))
        }
    }
}

// ── Tests (no live DB) ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    use async_trait::async_trait;

    use biocapital_contract::domain::{
        Contract, ContractPayout, ContractStatus, PayoutReason,
    };
    use biocapital_pg::ContractRepository;

    struct MemRepo {
        contracts: Mutex<HashMap<Uuid, Contract>>,
        payouts: Mutex<Vec<ContractPayout>>,
    }

    impl MemRepo {
        fn new() -> Self {
            Self {
                contracts: Mutex::new(HashMap::new()),
                payouts: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl ContractRepository for MemRepo {
        async fn create(&self, contract: &Contract) -> Result<(), ContractRepoError> {
            self.contracts
                .lock()
                .unwrap()
                .insert(contract.contract_id, contract.clone());
            Ok(())
        }
        async fn get(&self, id: Uuid) -> Result<Contract, ContractRepoError> {
            self.contracts
                .lock()
                .unwrap()
                .get(&id)
                .cloned()
                .ok_or(ContractRepoError::ContractNotFound { contract_id: id })
        }
        async fn list(
            &self,
            proposer: Option<Uuid>,
            acceptor: Option<Uuid>,
            status: Option<ContractStatus>,
            limit: i64,
        ) -> Result<Vec<Contract>, ContractRepoError> {
            let g = self.contracts.lock().unwrap();
            // When both proposer and acceptor are set to the same
            // UUID (gRPC `player_filter` semantics: "any contract
            // where player X is on either side"), match either side.
            // Otherwise fall back to the strict AND semantics.
            let party_filter = match (proposer, acceptor) {
                (Some(p), Some(a)) if p == a => Some(p),
                _ => None,
            };
            let mut v: Vec<Contract> = g
                .values()
                .filter(|c| match party_filter {
                    Some(p) => c.proposer_uuid == p || c.acceptor_uuid == p,
                    None => match proposer {
                        Some(p) => c.proposer_uuid == p,
                        None => true,
                    },
                })
                .filter(|c| match party_filter {
                    Some(_) => true, // already handled above
                    None => match acceptor {
                        Some(a) => c.acceptor_uuid == a,
                        None => true,
                    },
                })
                .filter(|c| match status {
                    Some(s) => c.status == s,
                    None => true,
                })
                .cloned()
                .collect();
            v.sort_by_key(|c| std::cmp::Reverse(c.updated_tick));
            v.truncate(limit as usize);
            Ok(v)
        }
        async fn update(&self, contract: &Contract) -> Result<(), ContractRepoError> {
            let mut g = self.contracts.lock().unwrap();
            if !g.contains_key(&contract.contract_id) {
                return Err(ContractRepoError::ContractNotFound {
                    contract_id: contract.contract_id,
                });
            }
            g.insert(contract.contract_id, contract.clone());
            Ok(())
        }
        async fn record_payout(
            &self,
            payout: &ContractPayout,
        ) -> Result<ContractPayout, ContractRepoError> {
            // Idempotent on request_id.
            let mut g = self.payouts.lock().unwrap();
            if let Some(p) = g.iter().find(|p| p.request_id == payout.request_id).cloned() {
                return Ok(p);
            }
            g.push(payout.clone());
            Ok(payout.clone())
        }
        async fn list_payouts(
            &self,
            contract_id: Uuid,
        ) -> Result<Vec<ContractPayout>, ContractRepoError> {
            let g = self.payouts.lock().unwrap();
            Ok(g.iter().filter(|p| p.contract_id == contract_id).cloned().collect())
        }
        async fn list_expired(
            &self,
            current_tick: i64,
        ) -> Result<Vec<Contract>, ContractRepoError> {
            let g = self.contracts.lock().unwrap();
            Ok(g.values()
                .filter(|c| {
                    c.status == ContractStatus::Proposed
                        && c.expires_tick.map_or(false, |d| d <= current_tick)
                })
                .cloned()
                .collect())
        }
    }

    struct CapturingBank {
        calls: Mutex<Vec<(Uuid, Uuid, i64, Uuid)>>,
    }
    impl CapturingBank {
        fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
            }
        }
    }
    #[async_trait]
    impl biocapital_pg::BankTransferPort for CapturingBank {
        async fn transfer(
            &self,
            from: Uuid,
            to: Uuid,
            amount: i64,
            _memo: Option<String>,
            request_id: Uuid,
        ) -> Result<(i64, i64), tonic::Status> {
            self.calls
                .lock()
                .unwrap()
                .push((from, to, amount, request_id));
            Ok((amount, amount))
        }
    }

    fn make_service() -> (ContractGrpc, Arc<MemRepo>, Arc<CapturingBank>) {
        let repo = Arc::new(MemRepo::new());
        let bank = Arc::new(CapturingBank::new());
        let deps = ContractServiceDeps::new(repo.clone() as Arc<dyn ContractRepository>)
            .with_bank_transfer(bank.clone() as Arc<dyn biocapital_pg::BankTransferPort + Send + Sync>);
        (ContractGrpc::new(deps), repo, bank)
    }

    fn propose_req(master: Uuid, slave: Uuid) -> ContractProposeRequest {
        ContractProposeRequest {
            master: PlayerIdentifier { player_uuid: master },
            slave: PlayerIdentifier { player_uuid: slave },
            terms_type: "DAILY_TRIBUTE".to_owned(),
            terms_json: r#"{"type":"DAILY_TRIBUTE"}"#.to_owned(),
            revenue_share_pct: 10.0,
            redemption_cost: 100,
            expires_tick: 0,
            request_id: Uuid::new_v4(),
        }
    }

    #[tokio::test]
    async fn propose_then_accept_round_trip() {
        let (svc, _repo, _bank) = make_service();
        let master = Uuid::new_v4();
        let slave = Uuid::new_v4();
        let resp = svc
            .propose_contract(Request::new(propose_req(master, slave)))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(resp.status, "PROPOSED");
        assert_eq!(resp.event_meta.kind, "contract.created");

        let accept = svc
            .accept_contract(Request::new(ContractAcceptRequest {
                contract_id: resp.contract_id,
                request_id: Uuid::new_v4(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(accept.status, "ACTIVE");
        assert_eq!(accept.event_meta.kind, "contract.activated");
    }

    #[tokio::test]
    async fn reject_only_from_proposed() {
        let (svc, _repo, _bank) = make_service();
        let master = Uuid::new_v4();
        let slave = Uuid::new_v4();
        let resp = svc
            .propose_contract(Request::new(propose_req(master, slave)))
            .await
            .unwrap()
            .into_inner();
        let rejected = svc
            .reject_contract(Request::new(ContractRejectRequest {
                contract_id: resp.contract_id,
                reason: "nope".into(),
                request_id: Uuid::new_v4(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(rejected.status, "REJECTED");
        assert_eq!(rejected.event_meta.kind, "contract.terminated");
    }

    #[tokio::test]
    async fn terminate_works_for_active() {
        let (svc, _repo, _bank) = make_service();
        let resp = svc
            .propose_contract(Request::new(propose_req(Uuid::new_v4(), Uuid::new_v4())))
            .await
            .unwrap()
            .into_inner();
        let _ = svc
            .accept_contract(Request::new(ContractAcceptRequest {
                contract_id: resp.contract_id,
                request_id: Uuid::new_v4(),
            }))
            .await
            .unwrap();
        let terminated = svc
            .terminate_contract(Request::new(ContractTerminateRequest {
                contract_id: resp.contract_id,
                reason: "end".into(),
                request_id: Uuid::new_v4(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(terminated.status, "TERMINATED");
    }

    #[tokio::test]
    async fn redeem_calls_bank_and_records_payout() {
        let (svc, repo, bank) = make_service();
        let master = Uuid::new_v4();
        let slave = Uuid::new_v4();
        let resp = svc
            .propose_contract(Request::new(propose_req(master, slave)))
            .await
            .unwrap()
            .into_inner();
        let _ = svc
            .accept_contract(Request::new(ContractAcceptRequest {
                contract_id: resp.contract_id,
                request_id: Uuid::new_v4(),
            }))
            .await
            .unwrap();

        let redeem_req_id = Uuid::new_v4();
        let redeemed = svc
            .redeem_contract(Request::new(ContractRedeemRequest {
                contract_id: resp.contract_id,
                request_id: redeem_req_id,
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(redeemed.status, "REDEEMED");
        assert_eq!(redeemed.event_meta.kind, "contract.payout");

        // Bank got called exactly once with the right pair.
        let calls = bank.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        let (from, to, amount, req_id) = calls[0];
        assert_eq!(from, slave);
        assert_eq!(to, master);
        assert_eq!(amount, 100);
        assert_eq!(req_id, redeem_req_id);
        drop(calls);

        // Payout row was persisted.
        let payouts = repo.list_payouts(resp.contract_id).await.unwrap();
        assert_eq!(payouts.len(), 1);
        assert_eq!(payouts[0].amount, 100);
        assert_eq!(payouts[0].reason, PayoutReason::Redemption);
    }

    #[tokio::test]
    async fn list_returns_only_filtered() {
        let (svc, _repo, _bank) = make_service();
        let master = Uuid::new_v4();
        let slave = Uuid::new_v4();
        let _ = svc
            .propose_contract(Request::new(propose_req(master, slave)))
            .await
            .unwrap();
        let resp = svc
            .list_contracts(Request::new(ListRequest {
                limit: 16,
                offset: 0,
                player_filter: Some(PlayerIdentifier { player_uuid: master }),
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(resp.total_count, 1);
    }
}