package mo.dystopia.biocapital.fluid;

import net.minecraft.core.BlockPos;
import net.minecraft.core.particles.ParticleOptions;
import net.minecraft.sounds.SoundEvent;
import net.minecraft.sounds.SoundEvents;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.LevelAccessor;
import net.minecraft.world.level.LevelReader;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.material.FluidState;
import net.neoforged.neoforge.fluids.BaseFlowingFluid;

import java.util.Optional;

/**
 * Base class shared by BioCapital fluids. Supplies sensible defaults for
 * behaviours every fluid in the mod needs (no infinite source conversion,
 * standard spread distance, vanilla bucket sound) and delegates the
 * mod-specific properties to {@link BaseFlowingFluid.Properties}.
 *
 * <p>Each concrete fluid is a pair of static inner classes:
 * {@code public static class Source extends AbstractBioFluid} paired with
 * {@code public static class Flowing extends AbstractBioFluid}. This matches
 * NeoForge's {@code BaseFlowingFluid} reference implementations.
 */
public abstract class AbstractBioFluid extends BaseFlowingFluid {

    protected AbstractBioFluid(Properties properties) {
        super(properties);
    }

    // ── Defaults every BioCapital fluid inherits ─────────────────

    @Override
    protected boolean canConvertToSource(Level level) {
        // None of the BioCapital fluids are infinite sources.
        return false;
    }

    @Override
    protected void beforeDestroyingBlock(LevelAccessor level, BlockPos pos, BlockState state) {
        // Default: no-op. Subclasses may override for fluid-specific block
        // interactions (e.g. dissolving sand around Charm Potion).
    }

    @Override
    protected int getSlopeFindDistance(LevelReader level) {
        return 4;
    }

    @Override
    protected int getDropOff(LevelReader level) {
        return 1;
    }

    @Override
    public int getTickDelay(LevelReader level) {
        return 5;
    }

    @Override
    protected float getExplosionResistance() {
        return 100.0f;
    }

    @Override
    public Optional<SoundEvent> getPickupSound() {
        return Optional.of(SoundEvents.BUCKET_FILL);
    }

    @Override
    protected ParticleOptions getDripParticle() {
        return null;
    }

    /**
     * Vanilla {@code FlowingFluid} declares a static {@code LEVEL}
     * integer property. The Flowing half of each fluid must register it
     * via {@code createFluidStateDefinition}; without this, the fluid
     * crashes on registry-load with "Cannot set property LEVEL as it does
     * not exist in <fluid>".
     *
     * <p>Both Source and Flowing subclasses call into this. The Source
     * override in vanilla adds a no-LEVEL state (level=0) by ignoring
     * the property; here we just include LEVEL for both since the
     * shared getAmount()/isSource() methods on the subclasses
     * differentiate the source (level=8) from flowing states.
     */
    @Override
    protected void createFluidStateDefinition(net.minecraft.world.level.block.state.StateDefinition.Builder<
            net.minecraft.world.level.material.Fluid, net.minecraft.world.level.material.FluidState> builder) {
        super.createFluidStateDefinition(builder);
        builder.add(net.minecraft.world.level.material.FlowingFluid.LEVEL);
    }
}
