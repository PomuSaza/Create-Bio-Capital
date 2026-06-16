package mo.dystopia.biocapital.item;

import net.minecraft.world.item.Item;

/**
 * Desire Fragment (欲望碎片) — small crystalline byproduct.
 *
 * <p>Stack size 512. Drops with a 3 % chance from any hostile mob kill
 * (configured in the global loot modifier; the drop itself is implemented
 * in the hostile-mob sub-task).
 */
public class DesireFragmentItem extends Item {
    public DesireFragmentItem() {
        super(new Properties()
                .stacksTo(512)
                .fireResistant());
    }
}
