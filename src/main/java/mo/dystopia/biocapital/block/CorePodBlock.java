package mo.dystopia.biocapital.block;

import com.simibubi.create.content.kinetics.base.DirectionalKineticBlock;
import com.simibubi.create.content.kinetics.base.IRotate;
import com.simibubi.create.foundation.block.IBE;
import mo.dystopia.biocapital.BioCapital;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.world.InteractionResult;
import net.minecraft.world.entity.LivingEntity;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.context.BlockPlaceContext;
import net.minecraft.world.level.BlockGetter;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.LevelAccessor;
import net.minecraft.world.level.LevelReader;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.entity.BlockEntity;
import net.minecraft.world.level.block.entity.BlockEntityTicker;
import net.minecraft.world.level.block.entity.BlockEntityType;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.StateDefinition;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;
import net.minecraft.world.level.block.state.properties.DoubleBlockHalf;
import net.minecraft.world.phys.shapes.CollisionContext;
import net.minecraft.world.phys.shapes.Shapes;
import net.minecraft.world.phys.shapes.VoxelShape;
import org.jetbrains.annotations.Nullable;

/**
 * Core Pod (核心舱) — a true 1×2×1 multi-block structure (per the design
 * blueprint). The block uses the standard {@link DoubleBlockHalf} pair
 * pattern: a {@code LOWER} half holds the BlockEntity and the SU output;
 * a {@code UPPER} half is a co-located decoration block placed at the
 * same time.
 *
 * <p>Both halves share the same registry entry and the same BlockItem;
 * the BlockItem always places the {@code LOWER} half, and the
 * {@code UPPER} half is auto-placed above it via {@link #setPlacedBy}.
 *
 * <p>Kinetic output: rotation axis along the {@link #FACING} direction;
 * the stress output face is the opposite side (see
 * {@link #hasShaftTowards}).
 */
public class CorePodBlock extends DirectionalKineticBlock implements IBE<CorePodBlockEntity> {

    /** Per-half 1×1×1 box; the visual model extends the upper half a
     *  bit further if you set it that way in the blockstate model. */
    public static final VoxelShape HALF_SHAPE = Shapes.box(0, 0, 0, 1, 1, 1);

    public CorePodBlock(Properties properties) {
        super(properties);
        this.registerDefaultState(this.stateDefinition.any()
                .setValue(FACING, Direction.NORTH)
                .setValue(BlockStateProperties.DOUBLE_BLOCK_HALF, DoubleBlockHalf.LOWER));
    }

    @Override
    protected void createBlockStateDefinition(StateDefinition.Builder<Block, BlockState> builder) {
        super.createBlockStateDefinition(builder);
        builder.add(BlockStateProperties.DOUBLE_BLOCK_HALF);
    }

    // ── Multi-block placement / survival ────────────────────────

    @Override
    public @Nullable BlockState getStateForPlacement(BlockPlaceContext context) {
        BlockPos pos = context.getClickedPos();
        Level level = context.getLevel();
        // Both halves' positions must be air (or fluid replaceable).
        BlockState above = level.getBlockState(pos.above());
        BlockState at    = level.getBlockState(pos);
        if (!at.isAir() && !at.canBeReplaced()) return null;
        if (!above.isAir() && !above.canBeReplaced()) return null;
        return this.defaultBlockState()
                .setValue(FACING, context.getHorizontalDirection().getOpposite())
                .setValue(BlockStateProperties.DOUBLE_BLOCK_HALF, DoubleBlockHalf.LOWER);
    }

    @Override
    public void setPlacedBy(Level level, BlockPos pos, BlockState state,
                            @Nullable LivingEntity placer, ItemStack stack) {
        super.setPlacedBy(level, pos, state, placer, stack);
        if (!level.isClientSide()
                && state.getValue(BlockStateProperties.DOUBLE_BLOCK_HALF) == DoubleBlockHalf.LOWER) {
            // Auto-place the UPPER half above, inheriting FACING.
            level.setBlock(pos.above(),
                    state.setValue(BlockStateProperties.DOUBLE_BLOCK_HALF, DoubleBlockHalf.UPPER),
                    Block.UPDATE_ALL);
        }
    }

    @Override
    protected boolean canSurvive(BlockState state, LevelReader level, BlockPos pos) {
        if (state.getValue(BlockStateProperties.DOUBLE_BLOCK_HALF) == DoubleBlockHalf.LOWER) {
            // LOWER: block above must be air, replaceable, OR our own UPPER half
            // (which setPlacedBy places via setBlock(..., UPDATE_ALL) — this
            // updateShape cascade would otherwise immediately remove the
            // freshly-placed LOWER).
            BlockPos above = pos.above();
            BlockState aboveState = level.getBlockState(above);
            if (aboveState.isAir() || aboveState.canBeReplaced()) return true;
            return aboveState.is(this)
                    && aboveState.getValue(BlockStateProperties.DOUBLE_BLOCK_HALF) == DoubleBlockHalf.UPPER
                    && aboveState.getValue(FACING) == state.getValue(FACING);
        }
        // UPPER: block below must be the LOWER half of the same block.
        BlockPos below = pos.below();
        BlockState belowState = level.getBlockState(below);
        return belowState.is(this)
                && belowState.getValue(BlockStateProperties.DOUBLE_BLOCK_HALF) == DoubleBlockHalf.LOWER
                && belowState.getValue(FACING) == state.getValue(FACING);
    }

    @Override
    protected BlockState updateShape(BlockState state, Direction direction,
                                     BlockState neighborState, LevelAccessor level,
                                     BlockPos pos, BlockPos neighborPos) {
        // If we're UPPER and the LOWER below us got removed, drop this half.
        if (direction == Direction.DOWN
                && state.getValue(BlockStateProperties.DOUBLE_BLOCK_HALF) == DoubleBlockHalf.UPPER
                && !(neighborState.is(this)
                     && neighborState.getValue(BlockStateProperties.DOUBLE_BLOCK_HALF) == DoubleBlockHalf.LOWER)) {
            return Blocks.AIR.defaultBlockState();
        }
        // If we're LOWER and the block above is no longer OK, drop this half too.
        if (direction == Direction.UP
                && state.getValue(BlockStateProperties.DOUBLE_BLOCK_HALF) == DoubleBlockHalf.LOWER
                && !canSurvive(state, (LevelReader) level, pos)) {
            return Blocks.AIR.defaultBlockState();
        }
        return super.updateShape(state, direction, neighborState, level, pos, neighborPos);
    }

    @Override
    public BlockState playerWillDestroy(Level level, BlockPos pos, BlockState state, Player player) {
        // Cascade: if breaking LOWER, also break UPPER; if breaking UPPER, just drop UPPER.
        DoubleBlockHalf half = state.getValue(BlockStateProperties.DOUBLE_BLOCK_HALF);
        BlockPos otherPos = (half == DoubleBlockHalf.LOWER) ? pos.above() : pos.below();
        BlockState otherState = level.getBlockState(otherPos);
        if (otherState.is(this)) {
            // Drop the OTHER half's item directly; don't run its full break path.
            // We use spawnAtLocation to give the right item to the player.
            if (!level.isClientSide() && !player.isCreative()) {
                // Mirror behaviour of DoublePlantBlock.preventDropFromBottomPart: only
                // the LOWER half drops the item, UPPER half drops nothing.
                if (half == DoubleBlockHalf.LOWER) {
                    Block.dropResources(state, level, pos, null, player, player.getMainHandItem());
                }
            }
            // Now break the other half (set to air) without further drops.
            level.setBlock(otherPos, Blocks.AIR.defaultBlockState(),
                    Block.UPDATE_CLIENTS | Block.UPDATE_SUPPRESS_DROPS);
        }
        return super.playerWillDestroy(level, pos, state, player);
    }

    // ── IRotate integration ─────────────────────────────────────

    @Override
    public Direction.Axis getRotationAxis(BlockState state) {
        return state.getValue(FACING).getAxis();
    }

    @Override
    public boolean hasShaftTowards(LevelReader world, BlockPos pos, BlockState state, Direction face) {
        return face == state.getValue(FACING).getOpposite();
    }

    // ── Visual / collision ──────────────────────────────────────

    @Override
    public VoxelShape getShape(BlockState state, BlockGetter level, BlockPos pos, CollisionContext context) {
        return HALF_SHAPE;
    }

    @Override
    public VoxelShape getCollisionShape(BlockState state, BlockGetter level, BlockPos pos, CollisionContext context) {
        return HALF_SHAPE;
    }

    // ── Right-click hosting (2026-06-14 task #119 cleanup) ──────────
    //
    // 原 CorePodHosting.enterPod / exitPod 业务逻辑（debuff / 位置约束 /
    // 耐久扣减）已下沉到 Rust 端 CorePodService.EnterPod / ExitPod（见
    // doc/04-core-pod.md §X + task #5 实现）。本类**只**做：
    //   1. 客户端立即返回 SUCCESS（防止按钮弹起感觉不到）
    //   2. 服务端转发 Sable JNI 调 Rust
    //   3. Rust 决策后通过 PlayerStateChangeEvent / CorePodStateChangeEvent
    //      反馈给 Java 端做后续状态同步
    //
    // 后续如需 enter/exit 状态变更的客户端提示，在 Rust gRPC 响应里附
    // `event_meta.kind`，Java 端 Sable JNI 接 event_meta 后调
    // `Minecraft.getInstance().player.displayClientMessage(...)`。

    @Override
    protected InteractionResult useWithoutItem(BlockState state, Level level, BlockPos pos,
                                             Player player, net.minecraft.world.phys.BlockHitResult hit) {
        if (level.isClientSide()) return InteractionResult.SUCCESS;
        if (!(level instanceof net.minecraft.server.level.ServerLevel)) {
            return InteractionResult.SUCCESS;
        }
        // Resolve the pod BE. The UPPER half has none; if the player clicked
        // the UPPER half, fall through to the LOWER half.
        BlockEntity be = level.getBlockEntity(pos);
        if (be == null
                && state.getValue(BlockStateProperties.DOUBLE_BLOCK_HALF) == DoubleBlockHalf.UPPER) {
            BlockState lower = level.getBlockState(pos.below());
            if (lower.is(this)
                    && lower.getValue(BlockStateProperties.DOUBLE_BLOCK_HALF) == DoubleBlockHalf.LOWER) {
                be = level.getBlockEntity(pos.below());
            }
        }
        if (!(be instanceof CorePodBlockEntity pod)) return InteractionResult.PASS;

        // 2026-06-14 task #5 + #119: 业务下沉 Rust。
        // 通过 Sable JNI callCorePod(CALL_CORE_POD, ENTER_POD=0 / EXIT_POD=1) 转发
        // CorePod 内部现在仅做侧感知 / 应力读取（task #79 SU=32/16 修正）。
        if (pod.getHostId().isEmpty()) {
            mo.dystopia.biocapital.NativeRustBindings.callCorePod(0 /* EnterPod */, new byte[0]);
            return InteractionResult.CONSUME;
        } else if (pod.getHostId().get().equals(player.getUUID())) {
            mo.dystopia.biocapital.NativeRustBindings.callCorePod(1 /* ExitPod */, new byte[0]);
            return InteractionResult.CONSUME;
        }
        return InteractionResult.FAIL; // occupied by someone else
    }

    // ── EntityBlock / IBE ───────────────────────────────────────
    // Only the LOWER half has the BlockEntity (and ticker).

    @Override
    public @Nullable BlockEntity newBlockEntity(BlockPos pos, BlockState state) {
        if (state.getValue(BlockStateProperties.DOUBLE_BLOCK_HALF) != DoubleBlockHalf.LOWER) {
            return null;
        }
        return BioCapital.CORE_POD_BE.get().create(pos, state);
    }

    @Override
    public @Nullable <T extends BlockEntity> BlockEntityTicker<T> getTicker(
            Level level, BlockState state, BlockEntityType<T> beType) {
        if (level.isClientSide()) return null;
        if (state.getValue(BlockStateProperties.DOUBLE_BLOCK_HALF) != DoubleBlockHalf.LOWER) return null;
        if (beType != BioCapital.CORE_POD_BE.get()) return null;
        return (lvl, pos, st, be) -> ((CorePodBlockEntity) be).serverTick();
    }

    @Override
    public Class<CorePodBlockEntity> getBlockEntityClass() {
        return CorePodBlockEntity.class;
    }

    @Override
    public BlockEntityType<? extends CorePodBlockEntity> getBlockEntityType() {
        return BioCapital.CORE_POD_BE.get();
    }

    public Direction getStressOutputFace(BlockState state) {
        return state.getValue(FACING).getOpposite();
    }
}
