package mo.dystopia.biocapital.fluid;

import mo.dystopia.biocapital.BioCapital;
import net.minecraft.core.registries.Registries;
import net.minecraft.world.item.BucketItem;
import net.minecraft.world.item.Item;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.LiquidBlock;
import net.minecraft.world.level.material.Fluid;
import net.neoforged.neoforge.fluids.BaseFlowingFluid;
import net.neoforged.neoforge.fluids.FluidType;
import net.neoforged.neoforge.registries.DeferredHolder;
import net.neoforged.neoforge.registries.DeferredRegister;

/**
 * Fluid registry — the Source (still) and Flowing variants of each fluid.
 *
 * <p>Cross-references between the still/flowing pair, the liquid block, the
 * bucket, and the {@link FluidType} are all expressed through suppliers so
 * the DeferredHolder graph can resolve them in any order at runtime. The
 * cross-references themselves live in private {@code makeXxxProps()} methods
 * rather than inline in the field initializers, so Java's
 * "self-reference in initializer" check never fires.
 *
 * <p><b>2026-06-14 task #8</b>: this class is now responsible only for
 * Minecraft-side fluid registration (the {@code DeferredHolder} graph for
 * the 4 mod fluids + their liquid blocks + bucket items + {@code FluidType}s).
 * The <i>actual effects</i> applied when a player interacts with or is
 * immersed in a fluid — pleasure / hunger / GENITAL part-dev / defeat /
 * core-pod stress — are driven by the Rust server from the
 * {@code fluid_effects} PostgreSQL table (see
 * {@code rust/crates/biocapital-core/src/fluids.rs},
 * {@code rust/crates/biocapital-pg/src/fluid.rs}, and
 * {@code rust/migrations/20260614000006_fluids.sql}).
 *
 * <p>The Java side is intentionally a <b>thin shim</b> over the Create
 * fluid network integration; runtime effect resolution happens server-side
 * via {@code PlayerStateService::add_fluid_effect} (CONSUMPTION path) and
 * {@code EnvironmentService::apply_fluid_effect_environmental}
 * (ENVIRONMENT path). Super Lubricant is a pure-decoration no-op per the
 * {@code 00 §4} user-decision override (it does <i>not</i> alter Create's
 * machine RPM cap, contrary to the original blueprint).
 */
public final class ModFluids {

    public static final DeferredRegister<Fluid> FLUIDS =
            DeferredRegister.create(Registries.FLUID, BioCapital.MODID);

    // ── High Tide ────────────────────────────────────────────────
    public static final DeferredHolder<Fluid, HighTideFluid.Source> HIGH_TIDE =
            FLUIDS.register("high_tide",
                    () -> new HighTideFluid.Source(makeHighTideProps()));
    public static final DeferredHolder<Fluid, HighTideFluid.Flowing> HIGH_TIDE_FLOWING =
            FLUIDS.register("high_tide_flowing",
                    () -> new HighTideFluid.Flowing(makeHighTideProps()));

    // ── Super Lubricant ──────────────────────────────────────────
    public static final DeferredHolder<Fluid, SuperLubricantFluid.Source> SUPER_LUBRICANT =
            FLUIDS.register("super_lubricant",
                    () -> new SuperLubricantFluid.Source(makeSuperLubricantProps()));
    public static final DeferredHolder<Fluid, SuperLubricantFluid.Flowing> SUPER_LUBRICANT_FLOWING =
            FLUIDS.register("super_lubricant_flowing",
                    () -> new SuperLubricantFluid.Flowing(makeSuperLubricantProps()));

    // ── Charm Potion ─────────────────────────────────────────────
    public static final DeferredHolder<Fluid, CharmPotionFluid.Source> CHARM_POTION =
            FLUIDS.register("charm_potion",
                    () -> new CharmPotionFluid.Source(makeCharmPotionProps()));
    public static final DeferredHolder<Fluid, CharmPotionFluid.Flowing> CHARM_POTION_FLOWING =
            FLUIDS.register("charm_potion_flowing",
                    () -> new CharmPotionFluid.Flowing(makeCharmPotionProps()));

    // ── Semen ────────────────────────────────────────────────────
    public static final DeferredHolder<Fluid, SemenFluid.Source> SEMEN =
            FLUIDS.register("semen",
                    () -> new SemenFluid.Source(makeSemenProps()));
    public static final DeferredHolder<Fluid, SemenFluid.Flowing> SEMEN_FLOWING =
            FLUIDS.register("semen_flowing",
                    () -> new SemenFluid.Flowing(makeSemenProps()));

    private ModFluids() {}

    public static void register() {
        // Fluid registrations are field-initialised.
    }

    // ── Properties factories (deferred-resolution lambdas) ───────

    private static BaseFlowingFluid.Properties makeHighTideProps() {
        return baseProps(ModFluidTypes.HIGH_TIDE_TYPE,
                HIGH_TIDE, HIGH_TIDE_FLOWING,
                mo.dystopia.biocapital.block.ModBlocks.HIGH_TIDE_BLOCK,
                mo.dystopia.biocapital.item.ModItems.HIGH_TIDE_BUCKET);
    }

    private static BaseFlowingFluid.Properties makeSuperLubricantProps() {
        return baseProps(ModFluidTypes.SUPER_LUBRICANT_TYPE,
                SUPER_LUBRICANT, SUPER_LUBRICANT_FLOWING,
                mo.dystopia.biocapital.block.ModBlocks.SUPER_LUBRICANT_BLOCK,
                mo.dystopia.biocapital.item.ModItems.SUPER_LUBRICANT_BUCKET);
    }

    private static BaseFlowingFluid.Properties makeCharmPotionProps() {
        return baseProps(ModFluidTypes.CHARM_POTION_TYPE,
                CHARM_POTION, CHARM_POTION_FLOWING,
                mo.dystopia.biocapital.block.ModBlocks.CHARM_POTION_BLOCK,
                mo.dystopia.biocapital.item.ModItems.CHARM_POTION_BUCKET);
    }

    private static BaseFlowingFluid.Properties makeSemenProps() {
        return baseProps(ModFluidTypes.SEMEN_TYPE,
                SEMEN, SEMEN_FLOWING,
                mo.dystopia.biocapital.block.ModBlocks.SEMEN_BLOCK,
                mo.dystopia.biocapital.item.ModItems.SEMEN_BUCKET);
    }

    /**
     * Build a {@link BaseFlowingFluid.Properties} bundle with the
     * liquid block and bucket attached. The four DeferredHolder
     * arguments keep cross-references clean: the Properties fields
     * store the DeferredHolders directly so {@code BaseFlowingFluid}
     * resolves them on demand rather than at construction time.
     */
    private static BaseFlowingFluid.Properties baseProps(
            DeferredHolder<FluidType, FluidType> fluidType,
            DeferredHolder<Fluid, ? extends Fluid> still,
            DeferredHolder<Fluid, ? extends Fluid> flowing,
            DeferredHolder<Block, net.minecraft.world.level.block.LiquidBlock> block,
            DeferredHolder<Item, net.minecraft.world.item.BucketItem> bucket) {
        return new BaseFlowingFluid.Properties(fluidType, still, flowing)
                .block(block)
                .bucket(bucket);
    }
}
