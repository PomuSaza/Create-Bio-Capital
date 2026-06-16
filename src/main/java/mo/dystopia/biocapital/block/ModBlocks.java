package mo.dystopia.biocapital.block;

import mo.dystopia.biocapital.BioCapital;
import mo.dystopia.biocapital.fluid.ModFluids;
import net.minecraft.core.registries.Registries;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.LiquidBlock;
import net.minecraft.world.level.block.state.BlockBehaviour;
import net.minecraft.world.level.material.FlowingFluid;
import net.minecraft.world.level.material.MapColor;
import net.neoforged.neoforge.registries.DeferredHolder;
import net.neoforged.neoforge.registries.DeferredRegister;

public final class ModBlocks {

    public static final DeferredRegister.Blocks BLOCKS =
            DeferredRegister.createBlocks(BioCapital.MODID);

    public static final DeferredHolder<Block, CorePodBlock> CORE_POD = BLOCKS.register(
            "core_pod",
            () -> new CorePodBlock(BlockBehaviour.Properties.of()
                    .mapColor(MapColor.COLOR_PURPLE)
                    .strength(6.0f, 12.0f)
                    .noOcclusion()
                    .lightLevel(s -> 4)));

    public static final DeferredHolder<Block, SwampMudBlock> SWAMP_MUD = BLOCKS.register(
            "swamp_mud",
            () -> new SwampMudBlock(BlockBehaviour.Properties.of()
                    .mapColor(MapColor.COLOR_BROWN)
                    .strength(0.6f)
                    .friction(0.85f)
                    .sound(net.minecraft.world.level.block.SoundType.MUD)));

    public static final DeferredHolder<Block, AtmBlock> ATM = BLOCKS.register(
            "atm",
            () -> new AtmBlock(BlockBehaviour.Properties.of()
                    .mapColor(MapColor.COLOR_GRAY)
                    .strength(3.0f, 6.0f)
                    .sound(net.minecraft.world.level.block.SoundType.METAL)
                    .lightLevel(s -> 6)));

    // ── Liquid blocks for each fluid ─────────────────────────────
    // Deferred-registered in ModBlocks so the BaseFlowingFluid.Properties
    // can hold a Supplier reference. The lambda body executes during the
    // BLOCK registry event, AFTER the FLUIDS event, so
    // ModFluids.X.get() returns the resolved fluid.

    public static final DeferredHolder<Block, LiquidBlock> HIGH_TIDE_BLOCK = BLOCKS.register(
            "high_tide", () -> new LiquidBlock(
                    (FlowingFluid) ModFluids.HIGH_TIDE.get(),
                    BlockBehaviour.Properties.of()
                            .mapColor(MapColor.COLOR_PINK)
                            .noCollission()
                            .strength(100.0f)
                            .liquid()
                            .replaceable()));

    public static final DeferredHolder<Block, LiquidBlock> SUPER_LUBRICANT_BLOCK = BLOCKS.register(
            "super_lubricant", () -> new LiquidBlock(
                    (FlowingFluid) ModFluids.SUPER_LUBRICANT.get(),
                    BlockBehaviour.Properties.of()
                            .mapColor(MapColor.COLOR_LIGHT_GREEN)
                            .noCollission()
                            .strength(100.0f)
                            .liquid()
                            .replaceable()));

    public static final DeferredHolder<Block, LiquidBlock> CHARM_POTION_BLOCK = BLOCKS.register(
            "charm_potion", () -> new LiquidBlock(
                    (FlowingFluid) ModFluids.CHARM_POTION.get(),
                    BlockBehaviour.Properties.of()
                            .mapColor(MapColor.COLOR_ORANGE)
                            .noCollission()
                            .strength(100.0f)
                            .liquid()
                            .lightLevel(s -> 8)
                            .replaceable()));

    public static final DeferredHolder<Block, LiquidBlock> SEMEN_BLOCK = BLOCKS.register(
            "semen", () -> new LiquidBlock(
                    (FlowingFluid) ModFluids.SEMEN.get(),
                    BlockBehaviour.Properties.of()
                            .mapColor(MapColor.SNOW)
                            .noCollission()
                            .strength(100.0f)
                            .liquid()
                            .replaceable()));

    private ModBlocks() {}

    public static void register() {
        // Each block is field-initialised; this method exists for symmetry.
    }
}
