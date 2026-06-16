package mo.dystopia.biocapital.block;

import com.simibubi.create.content.kinetics.base.GeneratingKineticBlockEntity;
import mo.dystopia.biocapital.BioCapital;
import mo.dystopia.biocapital.NativeRustBindings;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.HolderLookup;
import net.minecraft.core.UUIDUtil;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.level.block.entity.BlockEntity;
import net.minecraft.world.level.block.state.BlockState;
import net.neoforged.neoforge.capabilities.Capabilities;
import net.neoforged.neoforge.capabilities.RegisterCapabilitiesEvent;
import net.neoforged.neoforge.fluids.FluidStack;
import net.neoforged.neoforge.fluids.capability.IFluidHandler;
import net.neoforged.neoforge.fluids.capability.templates.FluidTank;
import net.neoforged.neoforge.items.ItemStackHandler;

import java.util.List;
import java.util.Optional;
import java.util.UUID;

/**
 * BlockEntity for the Core Pod — 2026-06-14 task #121 + #122 full rewrite.
 *
 * <p>Extends {@link GeneratingKineticBlockEntity} so the pod is automatically
 * wired into Create's kinetic network as a SOURCE: it generates rotation
 * (<b>32 RPM</b>) and contributes <b>16 SU</b> of capacity per cycle. SU
 * values are aligned with Create 6.0.10's {@code Steam Engine} (per
 * {@code doc/04-core-pod.md §2.2} — see also task #79 SU fix).
 *
 * <h2>Layout (per {@code doc/04-core-pod.md §1})</h2>
 * <ul>
 *   <li>1×2×1 multiblock (DOUBLE_BLOCK_HALF: LOWER + UPPER)</li>
 *   <li>Fluid <b>input</b> port: BOTTOM face</li>
 *   <li>Fluid <b>output</b> port: TOP face</li>
 *   <li>Stress <b>output</b>: HORIZONTAL (opposite of FACING)</li>
 *   <li>Item <b>byproduct</b> slot: FACING face (1 slot, capacity 64)</li>
 * </ul>
 *
 * <h2>Business logic delegation</h2>
 * <p>Per {@code doc/SYSTEM_PROMPT.md §11.1}: <strong>all production logic
 * lives in Rust</strong> ({@code rust/crates/biocapital-pod/}). This
 * BlockEntity is a thin Sable JNI shim:
 * <ul>
 *   <li>Hosts input/output fluid tanks + byproduct item slot (NeoForge
 *       capabilities for pipe/automation access).</li>
 *   <li>Tracks hosting state (player UUID + endurance) as a <em>cache</em>
 *       of the authoritative Rust state (PG {@code core_pods} table).</li>
 *   <li>Forwards SU capacity calculation to Rust via direct JNI
 *       ({@code computePodStress}, bypassing gRPC for hot-path latency).</li>
 *   <li>On server tick, fires {@code callCorePod(methodId=0=TickPod)} to let
 *       Rust drive the production state machine.</li>
 *   <li>Receives explicit events back from Rust (via
 *       {@code CorePodStateChangeEvent} / {@code CorePodProductionEvent})
 *       to update local cache state.</li>
 * </ul>
 *
 * <h2>Capability registration</h2>
 * <ul>
 *   <li>{@code Capabilities.FluidHandler.BLOCK} — side-aware (DOWN=input,
 *       UP=output, HORIZONTAL=combined)</li>
 *   <li>{@code Capabilities.ItemHandler.BLOCK} — byproduct slot on all 6 faces</li>
 * </ul>
 */
public class CorePodBlockEntity extends GeneratingKineticBlockEntity {

    /** Bucket size in millibuckets. */
    public static final int BUCKET_MB = 1000;
    /** Capacity of each fluid tank (one bucket). */
    public static final int TANK_CAPACITY = BUCKET_MB;
    /** Maximum endurance (in ticks). 20 minutes of continuous use at default. */
    public static final int MAX_ENDURANCE = 20 * 60 * 20;
    /** Byproduct slot capacity. */
    public static final int BYPRODUCT_SLOT_CAPACITY = 64;

    /**
     * Speed the pod generates, in RPM, while operational.
     * Aligned with Create 6.0.10 Steam Engine (doc/04-core-pod.md §2.2).
     */
    public static final float GENERATED_RPM = 32.0f;
    /**
     * Stress capacity the pod provides to the network.
     * Aligned with Create 6.0.10 Steam Engine (16 SU, doc/04-core-pod.md §2.2).
     */
    public static final float GENERATED_STRESS = 16.0f;
    /**
     * Stress the pod itself consumes (its own load).
     * Steam Engine has zero self-consumption.
     */
    public static final float SELF_STRESS = 0.0f;

    private final FluidTank inputTank = new FluidTank(TANK_CAPACITY) {
        @Override
        protected void onContentsChanged() {
            setChanged();
        }
    };
    private final FluidTank outputTank = new FluidTank(TANK_CAPACITY) {
        @Override
        protected void onContentsChanged() {
            setChanged();
        }
    };
    private final ItemStackHandler byproduct = new ItemStackHandler(1) {
        @Override
        protected void onContentsChanged(int slot) {
            setChanged();
        }
    };

    /** Empty = no host. When set, the pod is "running" with an occupant. */
    private Optional<UUID> hostId = Optional.empty();
    /** Remaining endurance in ticks. Decrements while a host is set. */
    private int endurance = MAX_ENDURANCE;

    public CorePodBlockEntity(BlockPos pos, BlockState state) {
        super(BioCapital.CORE_POD_BE.get(), pos, state);
    }

    // ── Create kinetic integration ──────────────────────────────

    @Override
    public float getGeneratedSpeed() {
        return GENERATED_RPM;
    }

    @Override
    public float calculateAddedStressCapacity() {
        // Direct-call SU computation via Sable JNI (bypasses gRPC for hot path).
        // Rust computes the per-pod stress aligned with Create 6.0.10 Steam Engine
        // (16 SU at 32 RPM, modulated by host/endurance per doc/04 §X).
        // doc/16-sable-bridge.md §3.4 specifies the JNI symbol
        // `Java_mo_dystopia_biocapital_NativeRustBindings_computePodStress0`.
        String hostUuid = getHostId().map(UUID::toString).orElse("");
        return NativeRustBindings.computePodStress(hostUuid, (int) getEndurance());
    }

    @Override
    public float calculateStressApplied() {
        return SELF_STRESS;
    }

    @Override
    public void addBehaviours(List<com.simibubi.create.foundation.blockEntity.behaviour.BlockEntityBehaviour> behaviours) {
        // No block-entity behaviours (no wrench goggle overlay yet).
    }

    // ── Capability registration ─────────────────────────────────

    /**
     * Register the pod's fluid tanks and item slot as NeoForge capabilities.
     * Fluid I/O is exposed side-aware: the BOTTOM face is the input port
     * (push fluid in), the TOP face is the output port (pull fluid out).
     * The other four horizontal faces expose both tanks for pipe-style
     * connections.
     */
    public static void registerCapabilities(RegisterCapabilitiesEvent event) {
        event.registerBlockEntity(
                Capabilities.FluidHandler.BLOCK,
                BioCapital.CORE_POD_BE.get(),
                (be, side) -> sideAwareFluid((CorePodBlockEntity) be, side));

        event.registerBlockEntity(
                Capabilities.ItemHandler.BLOCK,
                BioCapital.CORE_POD_BE.get(),
                (be, side) -> ((CorePodBlockEntity) be).byproduct);
    }

    private static IFluidHandler sideAwareFluid(CorePodBlockEntity be, @org.jetbrains.annotations.Nullable Direction side) {
        if (side == Direction.DOWN) return be.inputTank;
        if (side == Direction.UP)   return be.outputTank;
        // Horizontal faces: combined, so the caller picks the right slot.
        return new CombinedTankWrapper(be.inputTank, be.outputTank);
    }

    /**
     * Minimal two-tank wrapper: fill goes to the output tank (if compatible)
     * or input (otherwise); drain prefers output, then input. Mirrors the
     * original pre-rewrite semantics.
     */
    private static final class CombinedTankWrapper implements IFluidHandler {
        private final FluidTank in;
        private final FluidTank out;

        CombinedTankWrapper(FluidTank in, FluidTank out) {
            this.in = in;
            this.out = out;
        }

        @Override
        public int getTanks() { return 2; }

        @Override
        public FluidStack getFluidInTank(int tank) {
            return tank == 0 ? in.getFluid() : out.getFluid();
        }

        @Override
        public int getTankCapacity(int tank) {
            return tank == 0 ? in.getCapacity() : out.getCapacity();
        }

        @Override
        public boolean isFluidValid(int tank, FluidStack stack) {
            return tank == 0 ? in.isFluidValid(stack) : out.isFluidValid(stack);
        }

        @Override
        public int fill(FluidStack resource, FluidAction action) {
            int intoOut = out.fill(resource, action);
            if (intoOut >= resource.getAmount()) return intoOut;
            int remaining = resource.getAmount() - intoOut;
            FluidStack remainder = resource.copyWithAmount(remaining);
            return intoOut + in.fill(remainder, action);
        }

        @Override
        public FluidStack drain(FluidStack resource, FluidAction action) {
            FluidStack fromOut = out.drain(resource, action);
            if (!fromOut.isEmpty() && fromOut.getAmount() >= resource.getAmount()) return fromOut;
            int need = resource.getAmount() - fromOut.getAmount();
            FluidStack fromIn = in.drain(resource.copyWithAmount(need), action);
            FluidStack total = resource.copy();
            total.setAmount(fromOut.getAmount() + fromIn.getAmount());
            return total;
        }

        @Override
        public FluidStack drain(int maxDrain, FluidAction action) {
            FluidStack fromOut = out.drain(maxDrain, action);
            int need = maxDrain - fromOut.getAmount();
            if (need <= 0) return fromOut;
            FluidStack fromIn = in.drain(need, action);
            FluidStack total = fromOut.copy();
            total.grow(fromIn.getAmount());
            return total;
        }
    }

    // ── Capability accessors ─────────────────────────────────────

    public IFluidHandler getInputHandler() { return inputTank; }
    public IFluidHandler getOutputHandler() { return outputTank; }
    public ItemStackHandler getByproduct() { return byproduct; }

    public FluidTank getInputTank() { return inputTank; }
    public FluidTank getOutputTank() { return outputTank; }

    // ── Tick ─────────────────────────────────────────────────────

    /**
     * Server-side tick — drives Rust via Sable JNI.
     *
     * <p>Per {@code doc/04-core-pod.md §2.4} and {@code doc/SYSTEM_PROMPT.md §11.1}:
     * Java side does <strong>not</strong> run production logic, host debounce,
     * endurance decrement, or force-eject. All state transitions are
     * driven by Rust's {@code CorePodService.TickPod} (called via Sable JNI
     * here) and propagated back through {@code CorePodStateChangeEvent} /
     * {@code CorePodProductionEvent}.
     */
    public void serverTick() {
        if (level == null || level.isClientSide) return;
        if (!(level instanceof net.minecraft.server.level.ServerLevel)) return;

        // 2026-06-14 task #122: forward tick to Rust via Sable JNI.
        // Rust owns the state machine; this call is the entry point per tick.
        NativeRustBindings.callCorePod(0 /* TickPod */, new byte[0]);
    }

    // ── Hosting state (mirror of Rust PG row; updated via Sable JNI events) ──

    public Optional<UUID> getHostId() { return hostId; }
    public void setHost(UUID id) {
        this.hostId = Optional.ofNullable(id);
        setChanged();
    }
    public void clearHost() {
        this.hostId = Optional.empty();
        setChanged();
    }

    public int getEndurance() { return endurance; }
    public int getMaxEndurance() { return MAX_ENDURANCE; }
    public void setEndurance(int ticks) {
        this.endurance = Math.max(0, Math.min(MAX_ENDURANCE, ticks));
        setChanged();
    }

    // ── Persistence ──────────────────────────────────────────────

    @Override
    protected void write(CompoundTag tag, HolderLookup.Provider registries, boolean clientPacket) {
        super.write(tag, registries, clientPacket);
        CompoundTag inputNbt = new CompoundTag();
        inputTank.writeToNBT(registries, inputNbt);
        tag.put("input_tank", inputNbt);
        CompoundTag outputNbt = new CompoundTag();
        outputTank.writeToNBT(registries, outputNbt);
        tag.put("output_tank", outputNbt);
        tag.put("byproduct", byproduct.serializeNBT(registries));
        tag.putInt("endurance", endurance);
        hostId.ifPresent(id -> tag.put("host_id", UUIDUtil.CODEC.encodeStart(
                net.minecraft.nbt.NbtOps.INSTANCE, id).getOrThrow()));
    }

    @Override
    protected void read(CompoundTag tag, HolderLookup.Provider registries, boolean clientPacket) {
        super.read(tag, registries, clientPacket);
        if (tag.contains("input_tank"))  inputTank.readFromNBT(registries, tag.getCompound("input_tank"));
        if (tag.contains("output_tank")) outputTank.readFromNBT(registries, tag.getCompound("output_tank"));
        if (tag.contains("byproduct"))  byproduct.deserializeNBT(registries, tag.getCompound("byproduct"));
        if (tag.contains("endurance"))       endurance      = tag.getInt("endurance");
        if (tag.contains("host_id")) {
            // 2026-06-14 task #122: use parse (decode) for read; result()
            // returns Optional<UUID>; orElse(null) yields null on failure.
            UUID parsed = UUIDUtil.CODEC.parse(
                    net.minecraft.nbt.NbtOps.INSTANCE, tag.get("host_id"))
                    .result().orElse(null);
            hostId = Optional.ofNullable(parsed);
        } else {
            hostId = Optional.empty();
        }
    }
}
