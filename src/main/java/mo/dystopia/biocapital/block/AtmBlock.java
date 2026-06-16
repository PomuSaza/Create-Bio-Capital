package mo.dystopia.biocapital.block;

import com.mojang.serialization.MapCodec;
import com.simibubi.create.foundation.block.IBE;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.world.item.context.BlockPlaceContext;
import net.minecraft.world.level.BlockGetter;
import net.minecraft.world.level.block.DirectionalBlock;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.StateDefinition;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;
import net.minecraft.world.level.block.state.properties.DirectionProperty;
import net.minecraft.world.phys.shapes.CollisionContext;
import net.minecraft.world.phys.shapes.Shapes;
import net.minecraft.world.phys.shapes.VoxelShape;

/**
 * ATM block — 2026-06-14 task #119 cleanup.
 *
 * <p>原设计：virtualized crate (vault-style)，通过 Create 的 IItemHandler
 * 能力路由猫草 I/O（5 边进/出 + 1 边卡槽）。**不**打开 GUI。
 *
 * <p>现状：业务逻辑（卡槽、猫草进出、批次追踪）已下沉到 Rust
 * BankService + Sable JNI 钩子。本类**仅**保留方块注册 + 形状 +
 * 状态定义。**不**实现 EntityBlock / IBE 接口（没有 BlockEntity）。
 *
 * <p>如果后续需要 ATM 的 BlockEntity 重新支持（用于 capability 路由），
 * 请新建 AtmBlockEntity.java + 重新实现 IBE<AtmBlockEntity>。
 *
 * <p>参考：
 * <ul>
 *   <li>doc/08-bank.md §4 — ATM 行为规则</li>
 *   <li>doc/16-sable-bridge.md — Java↔Rust 桥</li>
 *   <li>doc/10-hardware-dglab.md — DG_LAB 协议（不适用 ATM）</li>
 * </ul>
 */
public class AtmBlock extends DirectionalBlock {

    public static final DirectionProperty FACING = BlockStateProperties.HORIZONTAL_FACING;

    public static final MapCodec<AtmBlock> CODEC =
            net.minecraft.world.level.block.state.BlockBehaviour.simpleCodec(AtmBlock::new);

    /** A bank-machine-style block, 1×1×1. */
    public static final VoxelShape SHAPE = Shapes.box(0, 0, 0, 1, 1, 1);

    public AtmBlock(Properties properties) {
        super(properties);
        this.registerDefaultState(this.stateDefinition.any().setValue(FACING, Direction.NORTH));
    }

    @Override
    protected MapCodec<? extends AtmBlock> codec() {
        return CODEC;
    }

    @Override
    protected void createBlockStateDefinition(StateDefinition.Builder<net.minecraft.world.level.block.Block, BlockState> builder) {
        super.createBlockStateDefinition(builder);
        builder.add(FACING);
    }

    @Override
    public BlockState getStateForPlacement(BlockPlaceContext context) {
        return this.defaultBlockState()
                .setValue(FACING, context.getHorizontalDirection().getOpposite());
    }

    @Override
    public VoxelShape getShape(BlockState state, BlockGetter level, BlockPos pos, CollisionContext context) {
        return SHAPE;
    }

    /**
     * Card slot is on the FACING face. Logic 已下沉 Rust；
     * NeoForge 事件通过 AtmBlockEntity 重新引入时再调用 IBE。
     */
    public Direction getCardFace(BlockState state) {
        return state.getValue(FACING);
    }
}
