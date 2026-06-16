//! Lifecycle transitions for `Contract` (09 §2.1 + 09 §3).
//!
//! Each public function is the single chokepoint for the
//! corresponding state-machine edge. The gRPC layer routes
//! every `ProposeContract` / `AcceptContract` / etc. RPC
//! through the helper here so the state machine stays
//! consistent regardless of caller (gRPC, JNI, future
//! scheduler, KubeJS injection).
//!
//! Transition graph (mirrors 09 §2.1 + user task #7 spec):
//!
//! ```text
//!                          (ProposeContract)
//!                                │
//!                                ▼
//!                            ┌────────┐  (auto by tokio task)
//!                            │PROPOSED│ ──── expires_tick ──► REJECTED
//!                            └────────┘
//!                                │
//!                                ▼ (AcceptContract by acceptor)
//!                            ┌────────┐
//!                            │ ACTIVE │
//!                            └────────┘
//!                              │    │
//!           (TerminateContract)│    │(RedeemContract by slave)
//!                              ▼    ▼
//!                       TERMINATED  REDEEMED
//!
//!  (RejectContract by acceptor from PROPOSED) ────► REJECTED
//! ```
//!
//! None of the helpers in this module touch the database — they
//! only validate the transition and mutate the in-memory struct.
//! The persistence layer (`biocapital-pg::contract`) reads the
//! returned struct and writes it.

use uuid::Uuid;

use super::contract::{
    Contract, ContractError, ContractPayout, ContractStatus, PayoutReason,
};

// ── propose_contract ────────────────────────────────────────────────────────

/// Build the initial `Contract` row from a `ProposeContract`
/// RPC. The status is forced to `PROPOSED`; the gRPC layer is
/// responsible for choosing the new `contract_id` (UUIDv7 in
/// production, but any UUID is acceptable per the SQL
/// `PRIMARY KEY` constraint).
///
/// `tick` is the server clock at proposal time. Stored on
/// `created_tick` and `updated_tick`; the latter is then
/// monotonically advanced by each subsequent lifecycle call.
pub fn propose_contract(
    contract_id: Uuid,
    proposer: Uuid,
    acceptor: Uuid,
    terms_type: String,
    terms_json: String,
    revenue_share_pct: f32,
    redemption_cost: i64,
    expires_tick: Option<i64>,
    tick: i64,
) -> Result<Contract, ContractError> {
    let contract = Contract {
        contract_id,
        proposer_uuid: proposer,
        acceptor_uuid: acceptor,
        status: ContractStatus::Proposed,
        terms_type,
        terms_json,
        revenue_share_pct,
        redemption_cost,
        expires_tick,
        created_tick: tick,
        updated_tick: tick,
        activated_tick: None,
        terminated_tick: None,
        redeemed_tick: None,
        reason: None,
    };
    contract.validate()?;
    Ok(contract)
}

// ── accept_contract ────────────────────────────────────────────────────────

/// `PROPOSED → ACTIVE`. The caller must be the `acceptor`
/// (slave); the proposer cannot self-accept their own
/// proposal. Expiry is re-checked at acceptance time so a
/// proposal that sat in `PROPOSED` past `expires_tick`
/// cannot be activated by a late click.
pub fn accept_contract(
    contract: &mut Contract,
    acceptor: Uuid,
    tick: i64,
) -> Result<(), ContractError> {
    if contract.acceptor_uuid != acceptor {
        return Err(ContractError::NotParty {
            contract_id: contract.contract_id,
            caller: acceptor,
            expected: "acceptor",
        });
    }
    if contract.status != ContractStatus::Proposed {
        return Err(ContractError::InvalidTransition {
            contract_id: contract.contract_id,
            current: contract.status,
            action: "accept",
        });
    }
    if contract.is_expired(tick) {
        // Best-effort cleanup — flip to REJECTED in-place so
        // the caller sees the actual state on their next
        // GetContract. The sweeper task will also catch any
        // stragglers.
        contract.status = ContractStatus::Rejected;
        contract.reason = Some("expired before accept".to_owned());
        contract.updated_tick = tick;
        contract.terminated_tick = Some(tick);
        return Err(ContractError::Expired {
            contract_id: contract.contract_id,
            expires_tick: contract.expires_tick.unwrap_or(tick),
        });
    }

    contract.status = ContractStatus::Active;
    contract.activated_tick = Some(tick);
    contract.updated_tick = tick;
    Ok(())
}

// ── reject_contract ────────────────────────────────────────────────────────

/// `PROPOSED → REJECTED`. Caller must be the `acceptor`
/// (slave). Once rejected the row is terminal — no further
/// transitions are possible.
pub fn reject_contract(
    contract: &mut Contract,
    caller: Uuid,
    reason: String,
    tick: i64,
) -> Result<(), ContractError> {
    if contract.acceptor_uuid != caller {
        return Err(ContractError::NotParty {
            contract_id: contract.contract_id,
            caller,
            expected: "acceptor",
        });
    }
    if contract.status != ContractStatus::Proposed {
        return Err(ContractError::InvalidTransition {
            contract_id: contract.contract_id,
            current: contract.status,
            action: "reject",
        });
    }

    contract.status = ContractStatus::Rejected;
    contract.reason = Some(truncate_reason(reason));
    contract.updated_tick = tick;
    contract.terminated_tick = Some(tick);
    Ok(())
}

// ── terminate_contract ─────────────────────────────────────────────────────

/// `ACTIVE → TERMINATED`. Either party can terminate an
/// active contract. The caller is recorded via `reason`
/// (the gRPC layer appends `by=<role>` to keep the audit
/// trail readable — see `terminate_reason` in
/// `99 §5.1.6`).
pub fn terminate_contract(
    contract: &mut Contract,
    caller: Uuid,
    reason: String,
    tick: i64,
) -> Result<(), ContractError> {
    let role = party_role(contract, caller).ok_or(ContractError::NotParty {
        contract_id: contract.contract_id,
        caller,
        expected: "proposer or acceptor",
    })?;
    if contract.status != ContractStatus::Active {
        return Err(ContractError::InvalidTransition {
            contract_id: contract.contract_id,
            current: contract.status,
            action: "terminate",
        });
    }

    contract.status = ContractStatus::Terminated;
    let mut reason = truncate_reason(reason);
    // Tag the actor role for the audit reader (Java UI surfaces
    // this on the contract detail page).
    reason = format!("by={role}; {reason}");
    contract.reason = Some(reason);
    contract.terminated_tick = Some(tick);
    contract.updated_tick = tick;
    Ok(())
}

// ── redeem_contract ────────────────────────────────────────────────────────

/// `ACTIVE → REDEEMED`. The caller must be the `proposer`
/// (master) — the master executes the redemption by
/// transferring `redemption_cost` from the slave (acceptor)
/// account to themselves. The actual ledger movement lives
/// in [`compute_payout`] and is performed by the gRPC layer
/// through `BankService.Transfer`; this helper only flips
/// the state machine and returns the pending payout row.
pub fn redeem_contract(
    contract: &mut Contract,
    redeemer: Uuid,
    tick: i64,
) -> Result<ContractPayout, ContractError> {
    if contract.proposer_uuid != redeemer {
        return Err(ContractError::NotParty {
            contract_id: contract.contract_id,
            caller: redeemer,
            expected: "proposer",
        });
    }
    if contract.status != ContractStatus::Active {
        return Err(ContractError::InvalidTransition {
            contract_id: contract.contract_id,
            current: contract.status,
            action: "redeem",
        });
    }

    let payout = compute_payout(contract, tick);
    contract.status = ContractStatus::Redeemed;
    contract.redeemed_tick = Some(tick);
    contract.updated_tick = tick;
    contract.reason = Some(format!(
        "redeemed at tick {tick} for {} cat-grass",
        contract.redemption_cost
    ));
    Ok(payout)
}

// ── compute_payout ─────────────────────────────────────────────────────────

/// Build the `ContractPayout` row that the redemption RPC
/// will hand to the bank layer. The actual transfer happens
/// in `biocapital_grpc::contract_service::ContractGrpc::redeem_contract`,
/// which calls `BankService.Transfer` with the `from` /
/// `to` accounts derived from the contract party UUIDs.
///
/// Note: this function does **not** look up bank account
/// UUIDs — the contract carries player UUIDs and the bank
/// resolver maps them. If the contract is between parties
/// who have never opened a bank account, the gRPC layer is
/// expected to auto-create the accounts on first use (see
/// `bank_service::ensure_account`).
///
/// `request_id` must be unique per redemption attempt —
/// replaying the same id hits the `idx_contract_payouts_request_id`
/// unique index and the bank transfer row is short-circuited
/// by `BankRepository::atomic_transfer`'s idempotency check.
pub fn compute_payout(contract: &Contract, tick: i64) -> ContractPayout {
    ContractPayout {
        payout_id: Uuid::new_v4(),
        contract_id: contract.contract_id,
        // Cross-crate mapping: from = slave (acceptor), to = master
        // (proposer). 09 §3.3 explicitly: "扣除成功后契约自动终止".
        from_account: contract.acceptor_uuid,
        to_account: contract.proposer_uuid,
        amount: contract.redemption_cost,
        reason: PayoutReason::Redemption,
        tick_millis: tick,
        request_id: Uuid::new_v4(),
    }
}

// ── helpers ────────────────────────────────────────────────────────────────

/// "PROPOSER" / "ACCEPTOR" / "NEITHER". Used by
/// `terminate_contract` to tag the actor in `reason` for the
/// audit reader.
fn party_role(contract: &Contract, caller: Uuid) -> Option<&'static str> {
    if caller == contract.proposer_uuid {
        Some("PROPOSER")
    } else if caller == contract.acceptor_uuid {
        Some("ACCEPTOR")
    } else {
        None
    }
}

/// SQL `contracts.reason` is VARCHAR(256); truncate rather
/// than fail. The gRPC layer's `reason` field is already
/// bounded by tonic's `String` length limits in practice.
fn truncate_reason(mut reason: String) -> String {
    const MAX: usize = 256;
    if reason.len() > MAX {
        reason.truncate(MAX);
        // Avoid splitting a UTF-8 codepoint at the boundary.
        while !reason.is_char_boundary(reason.len()) {
            reason.pop();
        }
    }
    reason
}

// ── Tests (no live DB) ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn build_contract(tick: i64) -> Contract {
        propose_contract(
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            "DAILY_TRIBUTE".to_owned(),
            r#"{"type":"DAILY_TRIBUTE"}"#.to_owned(),
            10.0,
            100,
            Some(1_000),
            tick,
        )
        .unwrap()
    }

    #[test]
    fn propose_rejects_self_contract() {
        let me = Uuid::new_v4();
        assert!(matches!(
            propose_contract(
                Uuid::new_v4(),
                me,
                me,
                "DAILY_TRIBUTE".to_owned(),
                "{}".to_owned(),
                10.0,
                100,
                None,
                0,
            ),
            Err(ContractError::SelfContract { .. })
        ));
    }

    #[test]
    fn propose_rejects_bad_pct() {
        assert!(matches!(
            propose_contract(
                Uuid::new_v4(),
                Uuid::new_v4(),
                Uuid::new_v4(),
                "DAILY_TRIBUTE".to_owned(),
                "{}".to_owned(),
                -1.0,
                100,
                None,
                0,
            ),
            Err(ContractError::RevenueShareOutOfRange { .. })
        ));
        assert!(matches!(
            propose_contract(
                Uuid::new_v4(),
                Uuid::new_v4(),
                Uuid::new_v4(),
                "DAILY_TRIBUTE".to_owned(),
                "{}".to_owned(),
                100.5,
                100,
                None,
                0,
            ),
            Err(ContractError::RevenueShareOutOfRange { .. })
        ));
    }

    #[test]
    fn propose_rejects_negative_redemption() {
        assert!(matches!(
            propose_contract(
                Uuid::new_v4(),
                Uuid::new_v4(),
                Uuid::new_v4(),
                "DAILY_TRIBUTE".to_owned(),
                "{}".to_owned(),
                10.0,
                -1,
                None,
                0,
            ),
            Err(ContractError::NegativeRedemptionCost { .. })
        ));
    }

    #[test]
    fn accept_only_by_acceptor() {
        let mut c = build_contract(0);
        let stranger = Uuid::new_v4();
        assert!(matches!(
            accept_contract(&mut c, stranger, 1),
            Err(ContractError::NotParty { .. })
        ));
    }

    #[test]
    fn accept_only_from_proposed() {
        let mut c = build_contract(0);
        let acceptor = c.acceptor_uuid;
        accept_contract(&mut c, acceptor, 1).unwrap();
        assert_eq!(c.status, ContractStatus::Active);
        assert_eq!(c.activated_tick, Some(1));

        // Second accept fails with InvalidTransition.
        assert!(matches!(
            accept_contract(&mut c, acceptor, 2),
            Err(ContractError::InvalidTransition { .. })
        ));
    }

    #[test]
    fn accept_after_expiry_rejects() {
        let mut c = build_contract(0);
        c.expires_tick = Some(50);
        let acceptor = c.acceptor_uuid;
        let err = accept_contract(&mut c, acceptor, 100).unwrap_err();
        assert!(matches!(err, ContractError::Expired { .. }));
        // Side effect: row should now be REJECTED.
        assert_eq!(c.status, ContractStatus::Rejected);
    }

    #[test]
    fn reject_only_from_proposed() {
        let mut c = build_contract(0);
        let acceptor = c.acceptor_uuid;
        reject_contract(&mut c, acceptor, "no".into(), 1).unwrap();
        assert_eq!(c.status, ContractStatus::Rejected);
        assert_eq!(c.reason.as_deref(), Some("no"));

        // Re-rejecting a REJECTED contract is rejected.
        assert!(matches!(
            reject_contract(&mut c, acceptor, "again".into(), 2),
            Err(ContractError::InvalidTransition { .. })
        ));
    }

    #[test]
    fn reject_truncates_reason_to_256() {
        let mut c = build_contract(0);
        let acceptor = c.acceptor_uuid;
        let huge = "x".repeat(1_000);
        reject_contract(&mut c, acceptor, huge.clone(), 1).unwrap();
        assert!(c.reason.as_ref().unwrap().len() <= 256);
    }

    #[test]
    fn terminate_by_either_party() {
        let mut c = build_contract(0);
        let acceptor = c.acceptor_uuid;
        accept_contract(&mut c, acceptor, 1).unwrap();
        // acceptor terminates
        terminate_contract(&mut c, acceptor, "end".into(), 2).unwrap();
        assert_eq!(c.status, ContractStatus::Terminated);
        assert_eq!(c.terminated_tick, Some(2));
        assert!(c.reason.as_ref().unwrap().contains("by=ACCEPTOR"));

        // proposer also terminates a separate contract
        let mut c2 = build_contract(0);
        let acceptor2 = c2.acceptor_uuid;
        let proposer2 = c2.proposer_uuid;
        accept_contract(&mut c2, acceptor2, 1).unwrap();
        terminate_contract(&mut c2, proposer2, "end".into(), 2).unwrap();
        assert_eq!(c2.status, ContractStatus::Terminated);
        assert!(c2.reason.as_ref().unwrap().contains("by=PROPOSER"));
    }

    #[test]
    fn terminate_rejects_stranger() {
        let mut c = build_contract(0);
        let acceptor = c.acceptor_uuid;
        accept_contract(&mut c, acceptor, 1).unwrap();
        assert!(matches!(
            terminate_contract(&mut c, Uuid::new_v4(), "x".into(), 2),
            Err(ContractError::NotParty { .. })
        ));
    }

    #[test]
    fn redeem_only_by_proposer() {
        let mut c = build_contract(0);
        let acceptor = c.acceptor_uuid;
        accept_contract(&mut c, acceptor, 1).unwrap();
        // acceptor cannot self-redeem
        assert!(matches!(
            redeem_contract(&mut c, acceptor, 2),
            Err(ContractError::NotParty { .. })
        ));
        // proposer can
        let proposer = c.proposer_uuid;
        let payout = redeem_contract(&mut c, proposer, 2).unwrap();
        assert_eq!(c.status, ContractStatus::Redeemed);
        assert_eq!(c.redeemed_tick, Some(2));
        assert_eq!(payout.amount, 100);
        assert_eq!(payout.contract_id, c.contract_id);
        assert_eq!(payout.from_account, c.acceptor_uuid);
        assert_eq!(payout.to_account, c.proposer_uuid);
        assert_eq!(payout.reason, PayoutReason::Redemption);
    }

    #[test]
    fn compute_payout_uses_redemption_cost() {
        let c = build_contract(0);
        let p = compute_payout(&c, 42);
        assert_eq!(p.amount, c.redemption_cost);
        assert_eq!(p.tick_millis, 42);
        assert_eq!(p.from_account, c.acceptor_uuid);
        assert_eq!(p.to_account, c.proposer_uuid);
    }
}