package mo.dystopia.biocapital.item;

import mo.dystopia.biocapital.BioCapital;
import mo.dystopia.biocapital.block.ModBlocks;
import mo.dystopia.biocapital.fluid.ModFluids;
import net.minecraft.core.registries.Registries;
import net.minecraft.world.item.BlockItem;
import net.minecraft.world.item.BucketItem;
import net.minecraft.world.item.Item;
import net.minecraft.world.item.Items;
import net.neoforged.neoforge.registries.DeferredHolder;
import net.neoforged.neoforge.registries.DeferredRegister;

public final class ModItems {

    public static final DeferredRegister<Item> ITEMS =
            DeferredRegister.createItems(BioCapital.MODID);

    public static final DeferredHolder<Item, DesireFragmentItem> DESIRE_FRAGMENT =
            ITEMS.register("desire_fragment", DesireFragmentItem::new);

    public static final DeferredHolder<Item, CatGrassItem> CAT_GRASS =
            ITEMS.register("cat_grass", CatGrassItem::new);

    public static final DeferredHolder<Item, BankCardItem> BANK_CARD =
            ITEMS.register("bank_card", BankCardItem::new);

    /** BlockItem for the Core Pod, so players can pick it up and place it. */
    public static final DeferredHolder<Item, BlockItem> CORE_POD_ITEM =
            ITEMS.register("core_pod", () -> new BlockItem(ModBlocks.CORE_POD.get(), new Item.Properties()));

    /** BlockItem for Swamp Mud, so it can be obtained in survival. */
    public static final DeferredHolder<Item, BlockItem> SWAMP_MUD_ITEM =
            ITEMS.register("swamp_mud", () -> new BlockItem(ModBlocks.SWAMP_MUD.get(), new Item.Properties()));

    /** BlockItem for the ATM, so players can place it. */
    public static final DeferredHolder<Item, BlockItem> ATM_ITEM =
            ITEMS.register("atm", () -> new BlockItem(ModBlocks.ATM.get(), new Item.Properties()));

    // ── Fluid buckets ────────────────────────────────────────────
    // Deferred-registered in ModItems so the BaseFlowingFluid.Properties
    // can hold a Supplier reference to them. The lambda body executes
    // during the ITEM registry event, AFTER the FLUIDS event, so
    // ModFluids.X.get() returns the resolved fluid.

    public static final DeferredHolder<Item, BucketItem> HIGH_TIDE_BUCKET =
            ITEMS.register("high_tide_bucket",
                    () -> new BucketItem(ModFluids.HIGH_TIDE.get(),
                            new Item.Properties().craftRemainder(Items.BUCKET).stacksTo(1)));

    public static final DeferredHolder<Item, BucketItem> SUPER_LUBRICANT_BUCKET =
            ITEMS.register("super_lubricant_bucket",
                    () -> new BucketItem(ModFluids.SUPER_LUBRICANT.get(),
                            new Item.Properties().craftRemainder(Items.BUCKET).stacksTo(1)));

    public static final DeferredHolder<Item, BucketItem> CHARM_POTION_BUCKET =
            ITEMS.register("charm_potion_bucket",
                    () -> new BucketItem(ModFluids.CHARM_POTION.get(),
                            new Item.Properties().craftRemainder(Items.BUCKET).stacksTo(1)));

    public static final DeferredHolder<Item, BucketItem> SEMEN_BUCKET =
            ITEMS.register("semen_bucket",
                    () -> new BucketItem(ModFluids.SEMEN.get(),
                            new Item.Properties().craftRemainder(Items.BUCKET).stacksTo(1)));

    private ModItems() {}

    public static void register() {
        // Items are field-initialised.
    }
}
