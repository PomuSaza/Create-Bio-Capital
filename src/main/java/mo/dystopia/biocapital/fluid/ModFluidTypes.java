package mo.dystopia.biocapital.fluid;

import mo.dystopia.biocapital.BioCapital;
import net.neoforged.neoforge.fluids.FluidType;
import net.neoforged.neoforge.registries.DeferredHolder;
import net.neoforged.neoforge.registries.DeferredRegister;
import net.neoforged.neoforge.registries.NeoForgeRegistries;

/**
 * Registry of {@link FluidType}s for BioCapital fluids.
 *
 * <p>FluidType is the modern (post-1.20) Forge replacement for the old
 * fluid metadata — it carries density, viscosity, light level, sounds,
 * rarity, and temperature, plus per-entity interaction flags (can drown,
 * can hydrate, etc.). The actual flow behaviour is in
 * {@link AbstractBioFluid} subclasses.
 *
 * <p>Each FluidType's description id is automatically taken from the
 * mod-id-prefixed translation key (e.g. {@code fluid.create_biocapital.high_tide}).
 */
public final class ModFluidTypes {

    public static final DeferredRegister<FluidType> FLUID_TYPES =
            DeferredRegister.create(NeoForgeRegistries.FLUID_TYPES, BioCapital.MODID);

    // ── High Tide (高潮流体) ─────────────────────────────────────
    public static final DeferredHolder<FluidType, FluidType> HIGH_TIDE_TYPE =
            FLUID_TYPES.register("high_tide", () -> new BioFluidType(
                    FluidType.Properties.create()
                            .density(1100)
                            .viscosity(1500)
                            .descriptionId("fluid." + BioCapital.MODID + ".high_tide"),
                    fluidTexture("high_tide_still"),
                    fluidTexture("high_tide_flow")));

    // ── Super Lubricant (超级润滑油) ─────────────────────────────
    public static final DeferredHolder<FluidType, FluidType> SUPER_LUBRICANT_TYPE =
            FLUID_TYPES.register("super_lubricant", () -> new BioFluidType(
                    FluidType.Properties.create()
                            .density(800)
                            .viscosity(100)
                            .descriptionId("fluid." + BioCapital.MODID + ".super_lubricant"),
                    fluidTexture("super_lubricant_still"),
                    fluidTexture("super_lubricant_flow")));

    // ── Charm Potion (媚药水体) ──────────────────────────────────
    public static final DeferredHolder<FluidType, FluidType> CHARM_POTION_TYPE =
            FLUID_TYPES.register("charm_potion", () -> new BioFluidType(
                    FluidType.Properties.create()
                            .density(1200)
                            .viscosity(2000)
                            .lightLevel(8)
                            .temperature(800)
                            .descriptionId("fluid." + BioCapital.MODID + ".charm_potion"),
                    fluidTexture("charm_potion_still"),
                    fluidTexture("charm_potion_flow")));

    // ── Semen (精液) ─────────────────────────────────────────────
    public static final DeferredHolder<FluidType, FluidType> SEMEN_TYPE =
            FLUID_TYPES.register("semen", () -> new BioFluidType(
                    FluidType.Properties.create()
                            .density(1500)
                            .viscosity(3000)
                            .descriptionId("fluid." + BioCapital.MODID + ".semen"),
                    fluidTexture("semen_still"),
                    fluidTexture("semen_flow")));

    private ModFluidTypes() {}

    /**
     * Reuse Minecraft's vanilla water textures as a placeholder for all
     * four BioCapital fluids — no PNG file needs to be supplied in
     * {@code assets/create_biocapital/textures/fluid/}. When real
     * per-fluid textures are available later, change this path to
     * {@code create_biocapital:fluid/<name>_still} (or remove this
     * helper entirely and inline the per-fluid {@link BioFluidType}
     * registrations).
     */
    private static net.minecraft.resources.ResourceLocation fluidTexture(String name) {
        return net.minecraft.resources.ResourceLocation.withDefaultNamespace("block/water_still");
    }
}
