package mo.dystopia.biocapital.state;

/**
 * Discrete body regions that have an independent "development" stat.
 *
 * <p>Per the design blueprint:
 * <ul>
 *   <li>FOOT — passive movement speed bonus
 *   <li>CHEST — increased stress output when operating hand cranks
 *   <li>ARM — extended interaction range
 *   <li>MOUTH — faster potion/drink consumption while gagged
 *   <li>ABDOMEN — increased hunger cap (HUD displays above 100 %)
 *   <li>GENITALS — placeholder hook for {@code JustARod} compatibility
 * </ul>
 *
 * <p>Each part has its own float value in {@link PlayerStateAttachment};
 * individual tuning logic is left as a separate concern.
 */
public enum BodyPart {
    FOOT,
    CHEST,
    ARM,
    MOUTH,
    ABDOMEN,
    GENITALS
}
