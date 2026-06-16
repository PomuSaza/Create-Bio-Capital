package mo.dystopia.biocapital.fluid;

import net.minecraft.core.particles.ParticleOptions;
import net.minecraft.core.particles.ParticleTypes;
import net.minecraft.world.level.material.FlowingFluid;
import net.minecraft.world.level.material.FluidState;
import net.neoforged.neoforge.fluids.BaseFlowingFluid.Properties;

/**
 * Super Lubricant (超级润滑油) — Create-mixer byproduct used to break
 * Create's gear rotational-speed ceiling.
 *
 * <p>Mix ratio: High Tide + crude oil in a Create mechanical mixer.
 * Low viscosity (100) and low density (800) so it floats on top of water
 * and runs off horizontal surfaces quickly.
 */
public abstract class SuperLubricantFluid extends AbstractBioFluid {

    protected SuperLubricantFluid(Properties properties) {
        super(properties);
    }

    @Override
    protected ParticleOptions getDripParticle() {
        return ParticleTypes.DRIPPING_HONEY;
    }

    public static class Source extends SuperLubricantFluid {
        public Source(Properties p) { super(p); }
        @Override public int getAmount(FluidState state) { return 8; }
        @Override public boolean isSource(FluidState state) { return true; }
    }

    public static class Flowing extends SuperLubricantFluid {
        public Flowing(Properties p) { super(p); }
        @Override
        public int getAmount(FluidState state) {
            return state.getValue(FlowingFluid.LEVEL);
        }
        @Override public boolean isSource(FluidState state) { return false; }
    }
}
