package mo.dystopia.biocapital.bank;

import mo.dystopia.biocapital.NativeRustBindings;

import java.util.Optional;
import java.util.UUID;

/**
 * Contract service stub — 2026-06-14 user decision: Java code was broken; rewrite properly.
 *
 * <p>Per `doc/SYSTEM_PROMPT.md` §11.1, this class retains only its public
 * method signatures and a thin Sable JNI dispatch to the Rust
 * {@code biocapital-contract::ContractService}. All business logic
 * (proposal, accept, reject, terminate, redeem, payout) lives exclusively
 * in Rust.
 *
 * <p>Real wiring:
 * <ul>
 *   <li>Rust gRPC server: {@code rust/crates/biocapital-contract/src/}</li>
 *   <li>PG tables: {@code contracts}, {@code contract_payouts}</li>
 * </ul>
 */
public final class ContractManager {

    /** Lifecycle states. Mirrors `biocapital_contract::domain::ContractStatus`. */
    public enum Status {
        PROPOSED,
        ACTIVE,
        TERMINATED,
        REDEEMED,
        REJECTED
    }

    /** Stub record; retained for legacy callers. Real data lives in Rust PG. */
    public record Contract(UUID contractId, UUID proposerUuid, UUID acceptorUuid,
                           Status status, String termsType, String termsJson,
                           float revenueSharePct, long redemptionCost,
                           long createdTick, long updatedTick) {}

    /**
     * Stub. Real implementation in {@code biocapital-contract::ContractService.ProposeContract}.
     */
    public Optional<Contract> proposeContract(UUID proposer, UUID acceptor, String termsType,
                                              String termsJson, float revenueSharePct,
                                              long redemptionCost) {
        NativeRustBindings.callContract(0 /* ProposeContract */, new byte[0]);
        return Optional.empty();
    }

    /**
     * Stub. Real implementation in {@code biocapital-contract::ContractService.AcceptContract}.
     */
    public Optional<Contract> acceptContract(UUID contractId, UUID acceptor) {
        NativeRustBindings.callContract(1 /* AcceptContract */, new byte[0]);
        return Optional.empty();
    }

    /**
     * Stub. Real implementation in {@code biocapital-contract::ContractService.RejectContract}.
     */
    public Optional<Contract> rejectContract(UUID contractId, String reason) {
        NativeRustBindings.callContract(2 /* RejectContract */, new byte[0]);
        return Optional.empty();
    }

    /**
     * Stub. Real implementation in {@code biocapital-contract::ContractService.TerminateContract}.
     */
    public Optional<Contract> terminateContract(UUID contractId, String reason) {
        NativeRustBindings.callContract(3 /* TerminateContract */, new byte[0]);
        return Optional.empty();
    }

    /**
     * Stub. Real implementation in {@code biocapital-contract::ContractService.RedeemContract}.
     */
    public Optional<Contract> redeemContract(UUID contractId, UUID redeemer) {
        NativeRustBindings.callContract(4 /* RedeemContract */, new byte[0]);
        return Optional.empty();
    }

    /**
     * Stub. Real implementation in {@code biocapital-contract::ContractService.GetContract}.
     */
    public Optional<Contract> getContract(UUID contractId) {
        NativeRustBindings.callContract(5 /* GetContract */, new byte[0]);
        return Optional.empty();
    }

    /**
     * Stub. Real implementation in {@code biocapital-contract::ContractService.ListContracts}.
     */
    public Contract[] listContracts(UUID player, Status statusFilter) {
        NativeRustBindings.callContract(6 /* ListContracts */, new byte[0]);
        return new Contract[0];
    }
}
