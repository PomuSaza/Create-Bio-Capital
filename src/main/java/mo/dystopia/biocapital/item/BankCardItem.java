package mo.dystopia.biocapital.item;

import mo.dystopia.biocapital.BioCapital;
import net.minecraft.core.UUIDUtil;
import net.minecraft.server.level.ServerPlayer;
import net.minecraft.world.InteractionHand;
import net.minecraft.world.InteractionResultHolder;
import net.minecraft.world.SimpleMenuProvider;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.item.Item;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.level.Level;

import java.util.UUID;

/**
 * Bank Card (银行卡) — identity token, NOT a wallet.
 *
 * <p>Carries an {@link #OWNER_UUID} data component that points to the
 * player who owns the account the card is bound to. Right-clicking the
 * card in the air opens the bank menu; the menu's actions are routed
 * to the account pointed to by this UUID.
 *
 * <p>Inserting the card into an ATM authenticates the ATM as a
 * physical proxy for the card's account: while the card is in the
 * ATM's slot, cat-grass I/O is credited/debited to that account.
 *
 * <h2>2026-06-14 task #4 (PRIORITY)</h2>
 * <p>Per {@code doc/08-bank.md} §3.5, the {@code device_lock} state
 * that gates cross-device transfers is the <strong>authoritative
 * truth</strong> in the Rust side ({@code bank_accounts.device_lock}
 * column). The DataComponent on this item is a display-only mirror
 * for the client; the gRPC {@code BankService.LockDevice} /
 * {@code BankService.UnlockDevice} RPCs are the writers. The Java
 * code path here must not bypass the Rust check.
 */
public class BankCardItem extends Item {

    public static final String DATA_OWNER_TAG = "OwnerUUID";

    /**
     * Set by {@code BioCapital}'s constructor (RegisterEvent listener
     * for the {@code data_component_type} registry) before the items
     * registry event fires. {@code static} but non-{@code final} so the
     * lambda can write to it.
     */
    public static net.minecraft.core.component.DataComponentType<UUID> OWNER_UUID;

    public BankCardItem() {
        super(new Properties().stacksTo(1).component(OWNER_UUID, new UUID(0L, 0L)));
    }

    // ── Helpers ──────────────────────────────────────────────────

    /** Card is "blank" if the embedded UUID is the all-zero default. */
    public static boolean isBlank(ItemStack stack) {
        UUID u = stack.get(OWNER_UUID);
        return u == null || (u.getLeastSignificantBits() == 0L
                           && u.getMostSignificantBits() == 0L);
    }

    public static UUID getOwner(ItemStack stack) {
        UUID u = stack.get(OWNER_UUID);
        return u == null ? new UUID(0L, 0L) : u;
    }

    public static ItemStack withOwner(ItemStack stack, UUID owner) {
        stack.set(OWNER_UUID, owner);
        return stack;
    }

    // ── Right-click in air → bank menu ──────────────────────────

    @Override
    public InteractionResultHolder<ItemStack> use(Level level, Player player, InteractionHand hand) {
        ItemStack stack = player.getItemInHand(hand);
        com.mojang.logging.LogUtils.getLogger().info("[BioCapital] BankCardItem.use() invoked: side={} player={}",
                level.isClientSide() ? "CLIENT" : "SERVER", player.getName().getString());
        if (level.isClientSide()) return InteractionResultHolder.success(stack);

        // If the card is blank, bind it to the current player first.
        if (isBlank(stack) && player instanceof ServerPlayer sp) {
            withOwner(stack, sp.getUUID());
        }

        if (player instanceof ServerPlayer sp) {
            UUID owner = getOwner(stack);
            com.mojang.logging.LogUtils.getLogger().info("[BioCapital] BankCardItem click (menu 已删, task #119)", owner);
            // 2026-06-14 task #119: BankMenu 已删。右键卡片不再打开游戏内 menu。
            // 后续如需游戏内 UI，重写 BankMenu；或使用 Web UI（doc/15-web-ui.md §X）作为权威前端。
            // 此处**仅**触发 Rust 端的 Sable JNI 钩子（如未来需要）。
            mo.dystopia.biocapital.NativeRustBindings.callBank(0 /* GetBalance */, new byte[0]);
            return InteractionResultHolder.consume(stack);
        }
        return InteractionResultHolder.pass(stack);
    }
}
