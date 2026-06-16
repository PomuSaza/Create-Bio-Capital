package mo.dystopia.biocapital.fluid;

import net.minecraft.core.particles.ParticleOptions;
import net.minecraft.core.particles.ParticleTypes;
import net.minecraft.world.level.material.FlowingFluid;
import net.minecraft.world.level.material.FluidState;
import net.neoforged.neoforge.fluids.BaseFlowingFluid.Properties;

/**
 * Charm Potion (媚药水体) — a lava-coloured, glowing, slow-draining
 * "aphrodisiac" pool.
 *
 * <p>Mix ratio: lava + High Tide in a Create mechanical mixer. Light level
 * 8 (glow). Any entity submerged accumulates pleasure and sheds other
 * debuffs. Implemented as a follow-up sub-task; this class only models the
 * fluid for recipe registration and world rendering.
 */
public abstract class CharmPotionFluid extends AbstractBioFluid {

    protected CharmPotionFluid(Properties properties) {
        super(properties);
    }

    @Override
    protected ParticleOptions getDripParticle() {
        return ParticleTypes.DRIPPING_LAVA;
    }

    public static class Source extends CharmPotionFluid {
        public Source(Properties p) { super(p); }
        @Override public int getAmount(FluidState state) { return 8; }
        @Override public boolean isSource(FluidState state) { return true; }
    }

    public static class Flowing extends CharmPotionFluid {
        public Flowing(Properties p) { super(p); }
        @Override
        public int getAmount(FluidState state) {
            return state.getValue(FlowingFluid.LEVEL);
        }
        @Override public boolean isSource(FluidState state) { return false; }
    }
}
