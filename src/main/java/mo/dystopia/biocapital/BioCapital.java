package mo.dystopia.biocapital;

import com.mojang.logging.LogUtils;
import mo.dystopia.biocapital.block.CorePodBlockEntity;
import mo.dystopia.biocapital.block.ModBlockEntities;
import mo.dystopia.biocapital.block.ModBlocks;
import mo.dystopia.biocapital.block.ModCreativeTabs;
import mo.dystopia.biocapital.fluid.ModFluidTypes;
import mo.dystopia.biocapital.fluid.ModFluids;
import mo.dystopia.biocapital.item.DesireFragmentItem;
import mo.dystopia.biocapital.item.ModItems;
import mo.dystopia.biocapital.state.PlayerStateAttachment;
import mo.dystopia.biocapital.world.ModEntities;
import net.minecraft.network.chat.Component;
import net.minecraft.resources.ResourceLocation;
import net.minecraft.world.item.Item;
import net.minecraft.world.level.block.entity.BlockEntityType;
import net.neoforged.bus.api.IEventBus;
import net.neoforged.fml.ModContainer;
import net.neoforged.fml.common.Mod;
import net.neoforged.fml.event.lifecycle.FMLCommonSetupEvent;
import net.neoforged.neoforge.attachment.AttachmentType;
import net.neoforged.neoforge.registries.DeferredHolder;
import net.neoforged.neoforge.registries.DeferredRegister;
import org.slf4j.Logger;

import java.util.UUID;

/**
 * Create: Bio-Capital — main mod entry point.
 *
 * <p>Mod id: {@value #MODID}. Group id: {@code mo.dystopia.biocapital}.<br>
 * Targets Minecraft {@code 1.21.1} on NeoForge, integrated with Create
 * {@code 6.0.x}.
 *
 * <p>2026-06-14 cleanup (task #119)：删除了 11 个 B 类 Java 业务文件
 * （AtmBlockEntity / CorePodBlockEntityRenderer / CorePodHosting /
 * CorePodClient / CreativeTabInjector / EnvironmentEffects /
 * HostileMobHandler / MobDropsHandler / BioCapitalHud / BankMenu /
 * BankScreen）。所有业务逻辑已下沉到 Rust 端（task #4-#14）。
 *
 * <p>本类现在只负责：注册（DeferredRegister）+ 启动监听 + JNI init +
 * 指令树 hint。**不**包含任何业务逻辑。
 */
@Mod(BioCapital.MODID)
public final class BioCapital {

    public static final String MODID = "create_biocapital";
    private static final Logger LOGGER = LogUtils.getLogger();

    /** Player-data attachments. Registered through {@link #ATTACHMENT_TYPES}. */
    public static final DeferredRegister<AttachmentType<?>> ATTACHMENT_TYPES =
            DeferredRegister.create(net.neoforged.neoforge.registries.NeoForgeRegistries.ATTACHMENT_TYPES, MODID);

    /** Data-component types (e.g. BankCard.OwnerUUID) need a
     *  registry entry or vanilla throws "Unregistered component". */
    public static final DeferredRegister<net.minecraft.core.component.DataComponentType<?>> DATA_COMPONENT_TYPES =
            DeferredRegister.create(net.minecraft.core.registries.Registries.DATA_COMPONENT_TYPE, MODID);
    // NOTE: DATA_COMPONENT_TYPES is no longer .register(modEventBus)'d in the
    // constructor — the BankCardItem OWNER_UUID field is now registered
    // via a direct RegisterEvent listener further down, so that the
    // assignment happens synchronously before the items registry event
    // fires (DeferredRegister fires its lambdas only when the relevant
    // event is dispatched, which is *after* the listener-adding
    // constructor returns; the order is wrong for a field that the
    // item constructor needs at construction time).

    public BioCapital(IEventBus modEventBus, ModContainer modContainer) {
        LOGGER.info("[BioCapital] Bootstrapping mod…");

        // Register the data-component type SYNCHRONOUSLY via a RegisterEvent
        // listener that fires before the items registry event. The field
        // BankCardItem.OWNER_UUID is then set; the items lambda can
        // reference it safely. We do NOT use DeferredRegister here
        // because the static class-load order would put the registration
        // AFTER the relevant RegisterEvent has already fired.
        modEventBus.addListener((net.neoforged.neoforge.registries.RegisterEvent event) -> {
            if (event.getRegistryKey() != net.minecraft.core.registries.Registries.DATA_COMPONENT_TYPE) return;
            event.register(net.minecraft.core.registries.Registries.DATA_COMPONENT_TYPE, helper -> {
                mo.dystopia.biocapital.item.BankCardItem.OWNER_UUID = net.minecraft.core.component.DataComponentType
                        .<UUID>builder()
                        .persistent(net.minecraft.core.UUIDUtil.CODEC)
                        .build();
                helper.register(
                        ResourceLocation.fromNamespaceAndPath(mo.dystopia.biocapital.BioCapital.MODID, "bank_card_owner"),
                        mo.dystopia.biocapital.item.BankCardItem.OWNER_UUID);
            });
        });

        // Content registers
        ModBlocks.BLOCKS.register(modEventBus);
        ModItems.ITEMS.register(modEventBus);
        ModFluids.FLUIDS.register(modEventBus);
        ModFluidTypes.FLUID_TYPES.register(modEventBus);
        ModBlockEntities.BLOCK_ENTITY_TYPES.register(modEventBus);
        ModEntities.ENTITY_TYPES.register(modEventBus);
        ModCreativeTabs.TABS.register(modEventBus);
        // ModMenuTypes 已删 BANK menu（task #119），MENU_TYPES 注册暂时停用

        // Attachments (player state)
        ATTACHMENT_TYPES.register(modEventBus);
        PlayerStateAttachment.register();

        // Capabilities (fluid, item) for BlockEntities
        // 2026-06-14 task #119: CorePodBlockEntity::registerCapabilities 保留
        // （CorePodBlock 仍在，CorePodBlockEntity 简化保留以处理侧感知）
        modEventBus.addListener(CorePodBlockEntity::registerCapabilities);
        // AtmBlockEntity::registerCapabilities 已删除（task #119）
        // 后续需要 ATM 能力时，新建简化 BlockEntity + 调 Rust BankService

        // 2026-06-14 task #119: CorePodClient 已删（无 Client 端渲染）
        // 客户端渲染如需要，重新创建 CorePodClient 类

        modEventBus.addListener(this::commonSetup);

        // External TOML config — load eagerly so HUD reads valid defaults.
        Config.load();

        // JNI bridge to native Rust services.  Best-effort: if the
        // biocapital_jni shared library is not on java.library.path the
        // call returns false and the mod continues in pure-Java /
        // pure-Rust-gRPC fallback mode (see NativeRustBindings.NATIVE_AVAILABLE
        // and doc/16-sable-bridge.md §3.5 + §10).
        if (!NativeRustBindings.init()) {
            LOGGER.warn("[BioCapital] Native bindings unavailable; falling back to Java/gRPC paths");
        }

        // 2026-06-14 task #13 (doc/12-command-system.md §3.1): 注册
        // /biocapital * 指令树到 NeoForge 事件总线。BiocapitalCommand
        // 自身使用 @SubscribeEvent 注解在 FORGE 总线上监听
        // RegisterCommandsEvent，此处不需要额外调用（类加载时
        // EventBusSubscriber 已自动注册）。
        LOGGER.info("[BioCapital] Command tree registration deferred to BiocapitalCommand @SubscribeEvent");
    }

    private void commonSetup(final FMLCommonSetupEvent event) {
        event.enqueueWork(() -> LOGGER.info("[BioCapital] Common setup complete"));
    }

    /** Convenience for {@code Component.translatable(MODID + "." + key)}. */
    public static Component translatable(String key, Object... args) {
        return Component.translatable(MODID + "." + key, args);
    }

    // ── Re-exports for cross-package convenience ─────────────────────

    /** Re-exported Desire Fragment item for use in {@code CorePodBlockEntity}. */
    public static final DeferredHolder<Item, DesireFragmentItem> DESIRE_FRAGMENT_ITEM =
            ModItems.DESIRE_FRAGMENT;

    /** Re-exported Cat Grass item. */
    public static final DeferredHolder<Item, ? extends Item> CAT_GRASS_ITEM =
            ModItems.CAT_GRASS;

    /** Re-exported Core Pod BlockEntity type. */
    public static final DeferredHolder<BlockEntityType<?>, BlockEntityType<CorePodBlockEntity>> CORE_POD_BE =
            ModBlockEntities.CORE_POD;

    // 2026-06-14 task #119: ATM_BE 已删（AtmBlockEntity 删了）
    // BANK_MENU 已删（BankMenu 删了）
    // BANK / CONTRACTS 已删（BankManager / ContractManager 业务下沉 Rust；保留类壳供 Sable JNI 钩子用）

    /** Server-side Rust-backed bank manager reference (业务下沉到 Rust). */
    public static final mo.dystopia.biocapital.bank.BankManager BANK =
            new mo.dystopia.biocapital.bank.BankManager();

    /** Server-side Rust-backed contract manager reference (业务下沉到 Rust). */
    public static final mo.dystopia.biocapital.bank.ContractManager CONTRACTS =
            new mo.dystopia.biocapital.bank.ContractManager();
}
