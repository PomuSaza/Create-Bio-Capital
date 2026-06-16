package mo.dystopia.biocapital.block;

import mo.dystopia.biocapital.BioCapital;
import net.minecraft.core.registries.Registries;
import net.minecraft.world.level.block.entity.BlockEntityType;
import net.neoforged.neoforge.registries.DeferredHolder;
import net.neoforged.neoforge.registries.DeferredRegister;

/**
 * Block-entity type registry. 2026-06-14 cleanup: ATM BlockEntity 已废弃
 * （task #119 删 AtmBlockEntity.java）；Core Pod BlockEntity 保留，
 * 但具体业务逻辑（compute_stress 等）已下沉 Rust（task #79 + task #110）。
 */
public final class ModBlockEntities {

    public static final DeferredRegister<BlockEntityType<?>> BLOCK_ENTITY_TYPES =
            DeferredRegister.create(Registries.BLOCK_ENTITY_TYPE, BioCapital.MODID);

    public static final DeferredHolder<BlockEntityType<?>, BlockEntityType<CorePodBlockEntity>> CORE_POD =
            BLOCK_ENTITY_TYPES.register("core_pod",
                    () -> BlockEntityType.Builder.of(CorePodBlockEntity::new, ModBlocks.CORE_POD.get())
                            .build(null));

    // ATM BlockEntity 已删除（task #119）。若后续需要再添加，
    // 请用 Rust 端 CorePodService 路由（见 doc/04-core-pod.md §X）。

    private ModBlockEntities() {}

    public static void register() {
        // Block-entity types are field-initialised.
    }
}
