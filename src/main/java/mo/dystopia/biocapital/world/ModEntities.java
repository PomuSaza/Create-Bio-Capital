package mo.dystopia.biocapital.world;

import mo.dystopia.biocapital.BioCapital;
import net.minecraft.core.registries.Registries;
import net.minecraft.world.entity.EntityType;
import net.neoforged.neoforge.registries.DeferredRegister;

/**
 * Entity-type registry.
 *
 * <p>2026-06-14 task #119 cleanup: HostileMobHandler / MobDropsHandler 已删
 * （业务下沉 Rust 端 mob_replacements 表 + HostileMobService gRPC）。本类
 * 仅保留 EntityType 注册入口。
 *
 * <p>2026-06-14 task #9: Java 端是 EntityType 类的权威注册源；Rust 端
 * {@code mob_replacements} 表是 (vanilla_id, creature_id, drop_chance,
 * priority, enabled) 路由的真值。两侧注册在运行时通过 creature_id
 * 字符串（snake_case，对应 Rust CreatureConfig.id）join。
 *
 * <p>后续如需 spawn 时路由，已通过 Sable JNI dispatch
 * {@code NativeRustBindings.callHostileMob} 处理。
 *
 * <p>参考：
 * <ul>
 *   <li>doc/06-hostile-mobs.md §X</li>
 *   <li>doc/13-bio-customization.md §X</li>
 *   <li>doc/16-sable-bridge.md §X</li>
 * </ul>
 */
public final class ModEntities {

    public static final DeferredRegister<EntityType<?>> ENTITY_TYPES =
            DeferredRegister.create(Registries.ENTITY_TYPE, BioCapital.MODID);

    private ModEntities() {}
}
