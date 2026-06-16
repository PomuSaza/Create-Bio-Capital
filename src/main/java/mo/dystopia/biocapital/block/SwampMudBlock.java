package mo.dystopia.biocapital.block;

import net.minecraft.core.BlockPos;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.LivingEntity;
import net.minecraft.world.level.BlockGetter;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.phys.Vec3;
import net.minecraft.world.phys.shapes.CollisionContext;
import net.minecraft.world.phys.shapes.Shapes;
import net.minecraft.world.phys.shapes.VoxelShape;

/**
 * Swamp Mud (沼泽) — a damp, soft block that generates in the surface
 * layer of {@code #minecraft:is_swamp} biomes. Walking across it is slow
 * (its friction is high; the player sinks in slightly). Used as the
 * surface decoration in the BioCapital swamp biome variant.
 *
 * <p>See {@code data/create_biocapital/neoforge/biome_modifier/swamp_mud.json}
 * for the worldgen wiring.
 */
public class SwampMudBlock extends Block {

    /** Full 1×1×1 box. */
    public static final VoxelShape SHAPE = Shapes.box(0, 0, 0, 1, 1, 1);

    public SwampMudBlock(Properties properties) {
        super(properties);
    }

    @Override
    public VoxelShape getShape(BlockState state, BlockGetter level, BlockPos pos, CollisionContext context) {
        return SHAPE;
    }

    @Override
    public VoxelShape getCollisionShape(BlockState state, BlockGetter level, BlockPos pos, CollisionContext context) {
        return SHAPE;
    }

    @Override
    public void entityInside(BlockState state, Level level, BlockPos pos, Entity entity) {
        // Soft push-up: sink a fraction of a block.
        if (entity instanceof LivingEntity) {
            entity.makeStuckInBlock(state, new Vec3(0.3, 0.6, 0.3));
        }
        super.entityInside(state, level, pos, entity);
    }
}
