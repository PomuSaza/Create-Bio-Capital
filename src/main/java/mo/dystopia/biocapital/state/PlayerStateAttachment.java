package mo.dystopia.biocapital.state;

import com.mojang.serialization.Codec;
import com.mojang.serialization.codecs.RecordCodecBuilder;
import mo.dystopia.biocapital.BioCapital;
import net.minecraft.world.entity.player.Player;
import net.neoforged.neoforge.attachment.AttachmentType;
import net.neoforged.neoforge.registries.DeferredHolder;
import net.neoforged.neoforge.registries.NeoForgeRegistries;

import java.util.EnumMap;
import java.util.Map;

/**
 * Per-player state backing the BioCapital mechanics.
 *
 * <p>This is a NeoForge <em>Attachment</em> — a key/value payload attached to
 * every {@link Player} entity. It is automatically persisted with the player
 * and cloned on respawn. Survives across sessions, just like vanilla data.
 *
 * <p>All public fields are deliberately mutable and the codec mirrors them
 * 1-to-1; the attachment holder never re-creates the struct in-place.
 *
 * <p><b>2026-06-14 task #3 (02-player-state.md):</b> 与 Rust
 * {@code rust/crates/biocapital-core/src/player_state.rs} 同步。本类的角色已
 * 降级为 <b>缓存镜像</b>：权威真值在 Rust 端的 PostgreSQL
 * {@code player_state} + {@code body_part_development} 两张表（doc/02 §4.1
 * + doc/99 §5）。本类仅在以下场景被读写：
 * <ul>
 *   <li>HUD 渲染（每 tick 客户端本地读）</li>
 *   <li>玩家离线时的本地暂存（02 §4.2）</li>
 *   <li>通过 Sable JNI 与 Rust gRPC 同步时的临时缓冲</li>
 * </ul>
 * 方法签名<b>不变</b>以保持 KubeJS / 现有事件总线兼容。
 */
public final class PlayerStateAttachment {

    // ── Constants ────────────────────────────────────────────────

    /** Default maximum for pleasure / hunger bars (display cap). */
    public static final float DEFAULT_STAT_MAX = 100.0f;

    /** Hidden HP cap; absorbed before the visible HUD reacts. */
    public static final float DEFAULT_HIDDEN_HP = 20.0f;

    /** Default starting values for fresh players. */
    public static final float SPAWN_PLEASURE = 0.0f;
    public static final float SPAWN_HUNGER    = 20.0f;
    public static final float SPAWN_HIDDEN_HP = DEFAULT_HIDDEN_HP;

    /** Minimum stat values; pleasure/hunger never go negative. */
    public static final float STAT_MIN = 0.0f;

    // ── Fields ───────────────────────────────────────────────────

    /** Pleasure (快感值). Increases from sweet berries, mob attacks, etc. */
    public float pleasure = SPAWN_PLEASURE;

    /** Hunger (饱食值). Replaces vanilla food level. */
    public float hunger = SPAWN_HUNGER;

    /**
     * Hidden HP (隐性血量). Vanilla HP is replaced by a backend value.
     * Damage is absorbed here first; when it would drop below 1 it is clamped
     * to 1, a counter is incremented, and a debuff is applied.
     */
    public float hiddenHp = SPAWN_HIDDEN_HP;

    /** Number of times the hidden HP floor has been hit (never resets to 0). */
    public int lowHpHits = 0;

    /** Per-body-part development values. Range and effects TBD per part. */
    public final Map<BodyPart, Float> partDevelopment = new EnumMap<>(BodyPart.class);

    // ── Constructors ─────────────────────────────────────────────

    public PlayerStateAttachment() {
        for (BodyPart p : BodyPart.values()) {
            partDevelopment.put(p, 0.0f);
        }
    }

    // ── Codec ────────────────────────────────────────────────────

    public static final Codec<PlayerStateAttachment> CODEC = RecordCodecBuilder.create(inst -> inst.group(
            Codec.FLOAT.optionalFieldOf("pleasure", SPAWN_PLEASURE).forGetter(s -> s.pleasure),
            Codec.FLOAT.optionalFieldOf("hunger",    SPAWN_HUNGER).forGetter(s -> s.hunger),
            Codec.FLOAT.optionalFieldOf("hiddenHp",  SPAWN_HIDDEN_HP).forGetter(s -> s.hiddenHp),
            Codec.INT.optionalFieldOf("lowHpHits",  0).forGetter(s -> s.lowHpHits),
            Codec.unboundedMap(Codec.STRING, Codec.FLOAT)
                    .optionalFieldOf("parts", Map.of())
                    .forGetter(s -> {
                        // Serialize the enum map keyed by part name for forward-compatibility.
                        java.util.HashMap<String, Float> out = new java.util.HashMap<>();
                        s.partDevelopment.forEach((k, v) -> out.put(k.name(), v));
                        return out;
                    })
    ).apply(inst, (pleasure, hunger, hiddenHp, lowHpHits, parts) -> {
        PlayerStateAttachment s = new PlayerStateAttachment();
        s.pleasure = clamp(pleasure, STAT_MIN, DEFAULT_STAT_MAX);
        s.hunger    = clamp(hunger,    STAT_MIN, DEFAULT_STAT_MAX);
        s.hiddenHp  = Math.max(1.0f, hiddenHp);
        s.lowHpHits = Math.max(0, lowHpHits);
        if (parts != null) {
            parts.forEach((name, val) -> {
                try {
                    BodyPart p = BodyPart.valueOf(name);
                    s.partDevelopment.put(p, Math.max(0.0f, val));
                } catch (IllegalArgumentException ignored) {
                    // Unknown body part in older saves; skip.
                }
            });
        }
        return s;
    }));

    // ── Registration ─────────────────────────────────────────────

    /** Attachment type registered through {@code BioCapital.ATTACHMENT_TYPES}. */
    public static final DeferredHolder<AttachmentType<?>, AttachmentType<PlayerStateAttachment>> TYPE =
            BioCapital.ATTACHMENT_TYPES.register("player_state", () ->
                    AttachmentType.builder(() -> new PlayerStateAttachment())
                            .serialize(PlayerStateAttachment.CODEC)
                            .build());

    /** Called from the mod constructor; trigger the registry binding. */
    public static void register() {
        // No-op: the registry binding happens at class-load time via the field
        // initializer above. This method is kept for symmetry with other
        // content classes (ModBlocks, ModItems, ...).
    }

    /**
     * Get the state attached to a player. Always non-null — the attachment
     * type's supplier guarantees an instance is created on first access.
     */
    public static PlayerStateAttachment get(Player player) {
        return player.getData(TYPE.get());
    }

    // ── Helpers ──────────────────────────────────────────────────

    public static float clamp(float v, float lo, float hi) {
        return Math.max(lo, Math.min(hi, v));
    }

    public void addPleasure(float delta) {
        pleasure = clamp(pleasure + delta, STAT_MIN, DEFAULT_STAT_MAX);
    }

    public void addHunger(float delta) {
        hunger = clamp(hunger + delta, STAT_MIN, DEFAULT_STAT_MAX);
    }

    /**
     * Apply damage to the hidden HP pool. Returns the actual amount absorbed
     * (which may be less than the input if the floor is hit).
     */
    public float applyHiddenDamage(float incoming) {
        if (incoming <= 0) return 0;
        float newHp = hiddenHp - incoming;
        if (newHp < 1.0f) {
            float absorbed = hiddenHp - 1.0f;
            hiddenHp = 1.0f;
            lowHpHits++;
            // Caller is responsible for applying the punitive debuff.
            return Math.max(0, absorbed);
        }
        hiddenHp = newHp;
        return incoming;
    }

    public void healHidden(float amount) {
        hiddenHp = Math.min(DEFAULT_HIDDEN_HP, hiddenHp + Math.max(0, amount));
    }

    public float getPart(BodyPart p) {
        return partDevelopment.getOrDefault(p, 0.0f);
    }

    public void addPart(BodyPart p, float delta) {
        partDevelopment.put(p, Math.max(0.0f, getPart(p) + delta));
    }

    public void setPart(BodyPart p, float value) {
        partDevelopment.put(p, Math.max(0.0f, value));
    }

    // ── 2026-06-14 task #3: Rust 权威真值 + JNI 转发钩子 ───────────
    //
    // 上述 {@code addPleasure / addHunger / applyHiddenDamage / healHidden /
    // addPart / setPart} 仍然在本类内部直接修改缓存字段（保证 KubeJS 脚本和
    // 现有事件总线 {@code PlayerStateChangeEvent} / {@code BodyPartChangeEvent}
    // 不被打断），但<b>权威真值在 Rust 端</b>：
    //
    //   触发 PlayerStateChangeEvent 实际由 Sable JNI
    //     callPlayerState(CALL_PLAYER_STATE_UPDATE, ...)
    //   转发到 Rust 的 PlayerStateService（doc/14 §3.2、doc/16 §3.4）。
    //   Rust 端在 gRPC 写完 PG 的 player_state + body_part_development 表后，
    //   再通过反向 dispatch 通知 Java 端刷新本 attachment（实现见
    //   rust/crates/biocapital-jni/src/dispatch.rs::player_state）。
    //
    // 在 setData() 之后（NeoForge Attachment 的反序列化入口，本类当前没有
    // 自定义 setData；如未来添加，注释位置应为该方法末尾）需要按上述链路
    // 显式触发一次 JNI 转发，避免 Java 缓存与 PG 真值长期背离。
}
