package mo.dystopia.biocapital.item;

import net.minecraft.network.chat.Component;
import net.minecraft.world.item.Item;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.TooltipFlag;

import java.util.List;

/**
 * Cat Grass (猫草) — the mod's fiat currency.
 *
 * <p>Per the v2 design blueprint (覆写原"NBT 标签 + 单格 1000"):
 * <ul>
 *   <li>No NBT / DataComponent on the item. Each blade is worth 1.</li>
 *   <li>Stack size 1000, so a full stack = 1000 units.</li>
 *   <li>Tracks the owner at the bank-card level (one card per account);
 *       physical cat grass is anonymous.</li>
 * </ul>
 *
 * <h2>2026-06-14 task #4 (PRIORITY)</h2>
 * <p>Per {@code doc/08-bank.md} §2.1 the cat-grass item carries
 * <strong>no NBT and no DataComponent</strong>. The batch_id used for
 * audit tracing lives only in the Rust-side
 * {@code cat_grass_batches} PostgreSQL table (08 §2.2 + 99 §5).
 * No code in this class should reach for a batch_id — look it up
 * through {@code NativeRustBindings.callBank} / the {@code BankService}
 * gRPC client instead.
 */
public class CatGrassItem extends Item {

    /** Max stack size, exposed for places (like the ATM) that withdraw
     *  a chunk of cat grass at a time. */
    public static final int MAX_STACK = 1000;

    public CatGrassItem() {
        super(new Properties().stacksTo(1000));
    }

    @Override
    public void appendHoverText(ItemStack stack, TooltipContext context, List<Component> tooltip, net.minecraft.world.item.TooltipFlag flag) {
        tooltip.add(Component.translatable("item.create_biocapital.cat_grass.tooltip",
                stack.getCount()));
    }
}
