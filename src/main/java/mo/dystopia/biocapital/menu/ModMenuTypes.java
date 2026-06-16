package mo.dystopia.biocapital.menu;

import mo.dystopia.biocapital.BioCapital;
import net.minecraft.core.registries.Registries;
import net.minecraft.world.inventory.MenuType;
import net.neoforged.neoforge.registries.DeferredRegister;

/**
 * Menu-type registry. 2026-06-14 cleanup: BANK menu 已删除（task #119
 * 删 BankMenu.java / BankScreen.java）。后续如需重写银行 UI，请用
 * Web UI 路由（doc/15-web-ui.md §X）作为权威前端。
 */
public final class ModMenuTypes {

    public static final DeferredRegister<MenuType<?>> MENU_TYPES =
            DeferredRegister.create(Registries.MENU, BioCapital.MODID);

    // BANK menu 已删除。如需重新添加，参考 doc/15-web-ui.md §X 的 Web UI 路径，
    // 优先选择 Web UI 而非游戏内 Menu。

    private ModMenuTypes() {}
}
