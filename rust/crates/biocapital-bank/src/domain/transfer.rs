//! Transfer validation — `08-bank.md` §3.4 面板 2 rules.
//!
//! The `validate_transfer` function is the single chokepoint every
//! gRPC `Transfer` call must pass through. It enforces:
//!   1. amount must be strictly positive
//!   2. source account must hold a non-zero balance
//!   3. amount cannot exceed the source's balance
//!   4. amount is capped at `MAX_BALANCE` (08 §3.4 rule 3 — large
//!      amounts get clamped rather than rejected)
//!   5. the device lock, if any, must match the presenting device
//!
//! Deposit / withdraw have their own validator helpers in this module
//! so the gRPC layer has a uniform `Result<…, BankError>` shape to
//! forward to the repository.

use uuid::Uuid;

use super::account::{BankAccount, BankError, MAX_BALANCE};

/// Validate a transfer request. Returns `Ok(capped_amount)` on
/// success; the capped amount is the value the caller should pass
/// downstream (i.e. equal to the requested amount, unless rule 4
/// clamped it).
///
/// `current_device_id` is the device presenting the request. Pass
/// `None` when the request arrives over a channel that is not
/// device-bound (e.g. an admin console) — the lock check then
/// rejects any locked account.
///
/// `request_id` is forwarded into the error variants so the gRPC
/// layer can log it for correlation.
pub fn validate_transfer(
    from: &BankAccount,
    amount: i64,
    current_device_id: Option<&str>,
    _request_id: Uuid,
) -> Result<i64, BankError> {
    if amount <= 0 {
        return Err(BankError::NonPositiveAmount(amount));
    }
    if from.balance == 0 {
        return Err(BankError::ZeroBalance);
    }
    if from.is_locked_to(current_device_id) {
        return Err(BankError::DeviceLocked {
            locked: from.device_lock.clone(),
            presented: current_device_id.map(str::to_owned),
        });
    }

    // 08 §3.4 rule 3: amount > MAX_BALANCE → cap to MAX_BALANCE
    // BEFORE the balance check. This is the only way a request for
    // `MAX_BALANCE + 1` can be capped rather than rejected as
    // overdraw (since `amount > from.balance` would otherwise trip
    // when the balance equals MAX_BALANCE). We then re-check the
    // capped value against the balance so a transfer of MAX_BALANCE
    // from an account that holds 50 units does not silently inflate
    // the source.
    let capped = amount.min(MAX_BALANCE);
    if capped > from.balance {
        return Err(BankError::InsufficientFunds {
            have: from.balance,
            want: capped,
        });
    }
    Ok(capped)
}

/// Validate a withdraw request. Mirrors the deposit / withdraw rules
/// on the bank screen; no device-lock check here because the Java
/// `BankMenu` already enforces the lock before the request is sent.
pub fn validate_withdraw(
    from: &BankAccount,
    amount: i64,
) -> Result<i64, BankError> {
    if amount <= 0 {
        return Err(BankError::NonPositiveAmount(amount));
    }
    if amount > from.balance {
        return Err(BankError::InsufficientFunds {
            have: from.balance,
            want: amount,
        });
    }
    Ok(amount.min(MAX_BALANCE).min(from.balance))
}

/// Validate a deposit. Returns `Ok(accepted_amount)` where
/// `accepted_amount <= amount` (the caller has to honour the cap and
/// only credit `accepted_amount` to the balance). `Ok(amount)` is the
/// common case; the cap kicks in only when the destination is
/// close to `max_balance`.
pub fn validate_deposit(
    to: &BankAccount,
    amount: i64,
) -> Result<i64, BankError> {
    if amount <= 0 {
        return Err(BankError::NonPositiveAmount(amount));
    }
    let headroom = to.max_balance - to.balance;
    if headroom <= 0 {
        return Err(BankError::OverMax {
            max: to.max_balance,
            have: to.balance,
            add: amount,
        });
    }
    Ok(amount.min(headroom))
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn account(balance: i64, max: i64, lock: Option<&str>) -> BankAccount {
        BankAccount {
            account_uuid: Uuid::new_v4(),
            owner_uuid: Uuid::new_v4(),
            balance,
            max_balance: max,
            device_lock: lock.map(str::to_owned),
            created_tick: 0,
            updated_tick: 0,
        }
    }

    #[test]
    fn transfer_rejects_zero_balance() {
        let a = account(0, MAX_BALANCE, None);
        let req = Uuid::new_v4();
        assert_eq!(
            validate_transfer(&a, 1, None, req),
            Err(BankError::ZeroBalance)
        );
    }

    #[test]
    fn transfer_rejects_overdraw() {
        let a = account(50, MAX_BALANCE, None);
        let req = Uuid::new_v4();
        assert_eq!(
            validate_transfer(&a, 100, None, req),
            Err(BankError::InsufficientFunds { have: 50, want: 100 })
        );
    }

    #[test]
    fn transfer_caps_at_max_balance() {
        let a = account(MAX_BALANCE, MAX_BALANCE, None);
        let req = Uuid::new_v4();
        // Request more than MAX_BALANCE; should be capped to balance.
        let capped = validate_transfer(&a, MAX_BALANCE + 1, None, req).unwrap();
        assert_eq!(capped, MAX_BALANCE);
    }

    #[test]
    fn transfer_rejects_device_mismatch() {
        let a = account(100, MAX_BALANCE, Some("dev-a"));
        let req = Uuid::new_v4();
        assert_eq!(
            validate_transfer(&a, 10, Some("dev-b"), req),
            Err(BankError::DeviceLocked {
                locked: Some("dev-a".to_owned()),
                presented: Some("dev-b".to_owned()),
            })
        );
    }

    #[test]
    fn transfer_succeeds_when_lock_matches() {
        let a = account(100, MAX_BALANCE, Some("dev-a"));
        let req = Uuid::new_v4();
        assert_eq!(validate_transfer(&a, 30, Some("dev-a"), req), Ok(30));
    }

    #[test]
    fn withdraw_rejects_zero_amount() {
        let a = account(10, MAX_BALANCE, None);
        assert_eq!(
            validate_withdraw(&a, 0),
            Err(BankError::NonPositiveAmount(0))
        );
    }

    #[test]
    fn deposit_caps_at_max_balance() {
        let a = account(MAX_BALANCE - 3, MAX_BALANCE, None);
        assert_eq!(validate_deposit(&a, 100).unwrap(), 3);
    }

    #[test]
    fn deposit_rejects_when_full() {
        let a = account(MAX_BALANCE, MAX_BALANCE, None);
        assert!(matches!(
            validate_deposit(&a, 1),
            Err(BankError::OverMax { .. })
        ));
    }
}
