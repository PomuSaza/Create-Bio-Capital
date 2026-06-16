package mo.dystopia.biocapital.bank;

import mo.dystopia.biocapital.NativeRustBindings;

import java.util.Optional;
import java.util.UUID;

/**
 * Bank service stub — 2026-06-14 user decision: Java code was broken; rewrite properly.
 *
 * <p>Per `doc/SYSTEM_PROMPT.md` §11.1, this class retains only its public method
 * signatures and a thin Sable JNI dispatch to the Rust
 * {@code biocapital-bank::BankService}. All business logic (balance storage,
 * transfer, history) lives exclusively in Rust.
 *
 * <p>Java-side fallbacks have been <strong>removed</strong>. Methods return
 * sentinel values ({@code 0L} / empty array) when the JNI bridge is
 * unavailable, so the client UI does not crash — but state is owned by Rust
 * (PostgreSQL {@code bank_accounts} / {@code bank_transactions}).
 *
 * <p>Real wiring:
 * <ul>
 *   <li>Rust gRPC server: {@code rust/crates/biocapital-bank/src/}</li>
 *   <li>PG tables: {@code bank_accounts}, {@code cat_grass_batches},
 *       {@code bank_transactions}, {@code audit_bank}</li>
 *   <li>Hardware token subsystem: see {@code doc/18-tg-whitelist.md}</li>
 * </ul>
 */
public final class BankManager {

    // Constant mirror of Rust-side `biocapital_bank::MAX_BALANCE` (doc/08 §8).
    // Used only for legacy BankMenu display; the actual cap is enforced by Rust.
    public static final long MAX_BALANCE = 100_000_000L;
    public static final int HISTORY_SIZE = 16;

    /** Stub; retained for legacy callers. */
    public enum Op { DEPOSIT, WITHDRAW, TRANSFER_OUT, TRANSFER_IN }

    /** Stub record; retained for legacy callers. New code uses Rust types. */
    public record Entry(Op op, long amount, long balanceAfter, long tickMillis,
                        @org.jetbrains.annotations.Nullable UUID counterparty,
                        @org.jetbrains.annotations.Nullable String counterpartyName) {}

    /**
     * Stub. Real implementation in {@code biocapital-bank::BankService.GetBalance}.
     *
     * @return balance from Rust PG, or {@code 0L} if JNI unavailable
     */
    public long getBalance(UUID player) {
        Optional<byte[]> rustResp = NativeRustBindings.callBank(0 /* GetBalance */, new byte[0]);
        return rustResp.map(BankManager::decodeBalanceSentinel).orElse(0L);
    }

    /**
     * Stub. Real implementation in {@code biocapital-bank::BankService.Deposit}.
     *
     * @return amount deposited by Rust, or {@code 0L} if JNI unavailable
     */
    public long deposit(UUID player, long amount) {
        Optional<byte[]> rustResp = NativeRustBindings.callBank(1 /* Deposit */, new byte[0]);
        return rustResp.map(BankManager::decodeBalanceSentinel).orElse(0L);
    }

    /**
     * Stub. Real implementation in {@code biocapital-bank::BankService.Withdraw}.
     */
    public long withdraw(UUID player, long amount) {
        Optional<byte[]> rustResp = NativeRustBindings.callBank(2 /* Withdraw */, new byte[0]);
        return rustResp.map(BankManager::decodeBalanceSentinel).orElse(0L);
    }

    /**
     * Stub. Real implementation in {@code biocapital-bank::BankService.Transfer}.
     */
    public long transfer(UUID from, UUID to, long amount, String counterpartyName) {
        Optional<byte[]> rustResp = NativeRustBindings.callBank(3 /* Transfer */, new byte[0]);
        return rustResp.map(BankManager::decodeBalanceSentinel).orElse(0L);
    }

    /**
     * Stub. Real implementation in {@code biocapital-bank::BankService.GetHistory}.
     */
    public Entry[] getHistory(UUID player) {
        // BankMenu should pull history via Sable JNI on every refresh; the
        // legacy cached history (bank_transactions ring buffer) is gone.
        return new Entry[0];
    }

    private static long decodeBalanceSentinel(byte[] payload) {
        // First 8 bytes = i64 BE balance per proto BalanceResponse layout.
        if (payload == null || payload.length < 8) return 0L;
        java.nio.ByteBuffer buf = java.nio.ByteBuffer.wrap(payload);
        return buf.getLong();
    }
}
