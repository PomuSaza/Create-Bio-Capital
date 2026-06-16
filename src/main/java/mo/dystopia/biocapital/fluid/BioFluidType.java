package mo.dystopia.biocapital.fluid;

import net.minecraft.resources.ResourceLocation;
import net.neoforged.neoforge.client.extensions.common.IClientFluidTypeExtensions;
import net.neoforged.neoforge.fluids.FluidType;

import java.util.function.Consumer;

/**
 * FluidType subclass that wires up client-side texture sprites for
 * the BioCapital fluids. Without this, the vanilla
 * {@code FluidSpriteCache.getFluidSprites} hits a null key in its
 * immutable map and crashes the world renderer as soon as one of our
 * fluid blocks is in view.
 *
 * <p>The default in 1.21 is to return {@code null} from
 * {@link IClientFluidTypeExtensions#getStillTexture()} and
 * {@code getFlowingTexture()}. That is the bug we're avoiding.
 *
 * <p>As a placeholder, this class points every BioCapital fluid at
 * vanilla water textures. The real per-fluid textures can be
 * supplied later by overriding this class per fluid, or by adding
 * {@code .png} files at the paths the code returns.
 */
public class BioFluidType extends FluidType {

    /** Where the still texture is loaded from for the given fluid id. */
    private final ResourceLocation stillTexture;
    /** Where the flowing texture is loaded from for the given fluid id. */
    private final ResourceLocation flowingTexture;

    public BioFluidType(Properties properties, ResourceLocation stillTexture, ResourceLocation flowingTexture) {
        super(properties);
        this.stillTexture  = stillTexture;
        this.flowingTexture = flowingTexture;
    }

    @Override
    public void initializeClient(Consumer<IClientFluidTypeExtensions> consumer) {
        consumer.accept(new IClientFluidTypeExtensions() {
            @Override
            public ResourceLocation getStillTexture() {
                return stillTexture;
            }

            @Override
            public ResourceLocation getFlowingTexture() {
                return flowingTexture;
            }
        });
    }
}
