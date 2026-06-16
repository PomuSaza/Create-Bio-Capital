package mo.dystopia.biocapital.fluid;

import net.minecraft.core.particles.ParticleOptions;
import net.minecraft.core.particles.ParticleTypes;
import net.minecraft.world.level.material.FlowingFluid;
import net.minecraft.world.level.material.FluidState;
import net.neoforged.neoforge.fluids.BaseFlowingFluid.Properties;

/**
 * Semen fluid (精液) — placeholder fluid for the just-a-rod compatibility
 * layer. Registered without a defined recipe or block; the visual / recipe
 * is left to a follow-up sub-task.
 */
public abstract class SemenFluid extends AbstractBioFluid {

    protected SemenFluid(Properties properties) {
        super(properties);
    }

    @Override
    protected ParticleOptions getDripParticle() {
        return ParticleTypes.DRIPPING_WATER;
    }

    public static class Source extends SemenFluid {
        public Source(Properties p) { super(p); }
        @Override public int getAmount(FluidState state) { return 8; }
        @Override public boolean isSource(FluidState state) { return true; }
    }

    public static class Flowing extends SemenFluid {
        public Flowing(Properties p) { super(p); }
        @Override
        public int getAmount(FluidState state) {
            return state.getValue(FlowingFluid.LEVEL);
        }
        @Override public boolean isSource(FluidState state) { return false; }
    }
}
