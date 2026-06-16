package mo.dystopia.biocapital.block;

import mo.dystopia.biocapital.BioCapital;
import net.minecraft.core.registries.Registries;
import net.minecraft.network.chat.Component;
import net.minecraft.world.item.CreativeModeTab;
import net.neoforged.neoforge.registries.DeferredHolder;
import net.neoforged.neoforge.registries.DeferredRegister;

/**
 * Creative tab registration. The tab uses vanilla {@link DeferredRegister}
 * to avoid the Registrate chicken-and-egg problem; items opt-in via
 * {@code .tab(() -> ModCreativeTabs.BIOCAPITAL_TAB.get())} at registration
 * time.
 */
public final class ModCreativeTabs {

    public static final DeferredRegister<CreativeModeTab> TABS =
            DeferredRegister.create(Registries.CREATIVE_MODE_TAB, BioCapital.MODID);

    public static final DeferredHolder<CreativeModeTab, CreativeModeTab> BIOCAPITAL_TAB = TABS.register("biocapital", () ->
            CreativeModeTab.builder()
                    .title(Component.translatable("itemGroup." + BioCapital.MODID))
                    .icon(() -> net.minecraft.world.item.Items.BARRIER.getDefaultInstance())
                    .build());

    private ModCreativeTabs() {}
}
