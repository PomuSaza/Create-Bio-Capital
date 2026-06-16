package mo.dystopia.biocapital.fluid;

import net.minecraft.core.particles.ParticleOptions;
import net.minecraft.core.particles.ParticleTypes;
import net.minecraft.world.level.material.FlowingFluid;
import net.minecraft.world.level.material.FluidState;
import net.neoforged.neoforge.fluids.BaseFlowingFluid.Properties;

/**
 * High Tide Fluid (高潮流体) — the primary BioCapital byproduct.
 *
 * <p>Heavier than water (viscosity 1500, density 1100). Spilled from the Core
 * Pod and from "climax" events. Thicker than lava when flowing — small
 * puddles spread slowly.
 */
public abstract class HighTideFluid extends AbstractBioFluid {

    protected HighTideFluid(Properties properties) {
        super(properties);
    }

    @Override
    protected ParticleOptions getDripParticle() {
        return ParticleTypes.DRIPPING_WATER;
    }

    public static class Source extends HighTideFluid {
        public Source(Properties p) { super(p); }
        @Override public int getAmount(FluidState state) { return 8; }
        @Override public boolean isSource(FluidState state) { return true; }
    }

    public static class Flowing extends HighTideFluid {
        public Flowing(Properties p) { super(p); }
        @Override
        public int getAmount(FluidState state) {
            return state.getValue(FlowingFluid.LEVEL);
        }
        @Override public boolean isSource(FluidState state) { return false; }
    }
}
