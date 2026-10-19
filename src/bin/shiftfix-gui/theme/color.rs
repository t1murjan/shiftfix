//! Расчёт цвета: контраст по WCAG 2, чёрный или белый текст на акценте,
//! строгий разбор «#rrggbb», второй цвет градиента.

use eframe::egui::{Color32, ecolor};

use super::palettes::{BLACK, Palette, WHITE, rgb};

/// Относительная яркость цвета по WCAG 2.
fn luminance(c: Color32) -> f32 {
    let lin = ecolor::linear_f32_from_gamma_u8;
    0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
}

/// Контраст двух цветов по WCAG 2: от 1 (одинаковые) до 21 (чёрный и белый).
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// Чёрный или белый — что контрастнее на этом цвете.
pub fn on_accent(c: Color32) -> Color32 {
    best_on(c, c)
}

/// Чёрный или белый для надписей на градиенте `from → to`: у кого худший
/// из двух контрастов выше.
pub fn best_on(from: Color32, to: Color32) -> Color32 {
    let worst = |ink| contrast(ink, from).min(contrast(ink, to));
    if worst(BLACK) >= worst(WHITE) {
        BLACK
    } else {
        WHITE
    }
}

/// Строгий разбор «#rrggbb»: ровно 6 шестнадцатеричных цифр, «#» по желанию.
/// Короткая запись и альфа не принимаются.
pub fn parse_hex(s: &str) -> Option<Color32> {
    let s = s.trim();
    let digits = s.strip_prefix('#').unwrap_or(s);
    if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(digits, 16).ok().map(rgb)
}

/// Цвет в виде «#rrggbb» — так он записывается в config.toml.
pub fn format_hex(c: Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b())
}

/// Сдвиг оттенка второго цвета градиента от своего акцента: как у Breeze,
/// где после голубого идёт фиолетовый (около 54°).
const ACCENT2_HUE_SHIFT: f32 = 0.15;

/// Второй цвет градиента для своего или системного акцента: тот же цвет
/// (HSV в гамма-пространстве) со сдвигом оттенка.
pub(super) fn shifted_hue(c: Color32) -> Color32 {
    let (h, s, v) = ecolor::hsv_from_rgb([c.r(), c.g(), c.b()].map(|x| f32::from(x) / 255.0));
    let [r, g, b] = ecolor::rgb_from_hsv(((h + ACCENT2_HUE_SHIFT).fract(), s, v));
    let u8_of = |x: f32| (x * 255.0).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(u8_of(r), u8_of(g), u8_of(b))
}

/// Ниже этого контраста с фоном карточек акцентные значки, заголовки и
/// ссылки читаются плохо. Все встроенные палитры порог проходят (тест):
/// у Breeze light акцент к белой карточке — около 3.04.
pub const ACCENT_MIN_CONTRAST: f32 = 3.0;

/// Акцент виден на фоне карточек.
pub fn accent_readable(p: &Palette) -> bool {
    contrast(p.accent, p.card) >= ACCENT_MIN_CONTRAST
}

/// Ручка, слишком похожая на дорожку, теряется.
pub(super) const KNOB_MIN_CONTRAST: f32 = 1.5;

/// Ручка включённого переключателя: цвет палитры, а на похожем на него
/// своём акценте — чёрная или белая.
pub fn knob_on(p: &Palette) -> Color32 {
    if contrast(p.knob, p.accent) >= KNOB_MIN_CONTRAST {
        p.knob
    } else {
        p.on_accent
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::find;

    #[test]
    fn on_accent_by_contrast() {
        assert_eq!(on_accent(WHITE), BLACK);
        assert_eq!(on_accent(BLACK), WHITE);
        assert_eq!(on_accent(rgb(0xffff00)), BLACK);
        assert_eq!(on_accent(rgb(0x0000ff)), WHITE);
        // Breeze: по WCAG на обоих акцентах читается чёрный, а не белый.
        assert_eq!(on_accent(rgb(0x3daee9)), BLACK);
        assert_eq!(on_accent(rgb(0x1d99f3)), BLACK);
    }

    #[test]
    fn on_accent_threshold() {
        // Порог — яркость около 0.179, где контраст с чёрным и с белым равны.
        assert_eq!(on_accent(rgb(0x757575)), WHITE);
        assert_eq!(on_accent(rgb(0x767676)), BLACK);
    }

    #[test]
    fn contrast_range() {
        assert!((contrast(BLACK, WHITE) - 21.0).abs() < 0.01);
        assert!((contrast(WHITE, BLACK) - 21.0).abs() < 0.01);
        assert!((contrast(rgb(0x3daee9), rgb(0x3daee9)) - 1.0).abs() < 1e-6);
        // Breeze light: акцент на белой карточке — чуть выше 3.
        let c = contrast(rgb(0x1d99f3), WHITE);
        assert!((3.0..3.1).contains(&c), "{c}");
    }

    #[test]
    fn best_on_gradient() {
        // Заголовок «Работает» у Breeze: белый на голубом даёт 2.5, чёрный — 4.5.
        assert_eq!(best_on(rgb(0x3daee9), rgb(0x7c5ce0)), BLACK);
        // Тёмный градиент — белые надписи.
        assert_eq!(best_on(rgb(0x1e3a8a), rgb(0x4c1d95)), WHITE);
        // Решает худший конец: на синем лучше белый, но на жёлтом он не виден.
        assert_eq!(on_accent(rgb(0x0000ff)), WHITE);
        assert_eq!(best_on(rgb(0xffff00), rgb(0x0000ff)), BLACK);
    }

    #[test]
    fn knob_on_own_accent() {
        let mut p = find("breeze").dark;
        // Свой белый акцент — белая ручка потерялась бы, берём чёрную.
        p.accent = WHITE;
        p.on_accent = on_accent(p.accent);
        assert_eq!(knob_on(&p), BLACK);
        p.accent = rgb(0x1e3a8a);
        p.on_accent = on_accent(p.accent);
        assert_eq!(knob_on(&p), p.knob);
    }

    #[test]
    fn hex_strict() {
        let orange = rgb(0xff8800);
        assert_eq!(parse_hex("#ff8800"), Some(orange));
        assert_eq!(parse_hex("ff8800"), Some(orange));
        assert_eq!(parse_hex("#FF8800"), Some(orange));
        assert_eq!(parse_hex(" #ff8800 "), Some(orange));
        for bad in [
            "",
            "#",
            "#f80",
            "f80",
            "#ff880080",
            "#ff88",
            "#ff88zz",
            "##ff8800",
            "+ff880",
            "#ff 880",
            "#ффф",
        ] {
            assert_eq!(parse_hex(bad), None, "{bad:?}");
        }
        assert_eq!(format_hex(orange), "#ff8800");
        assert_eq!(format_hex(rgb(0x0a0b0c)), "#0a0b0c");
        for c in [orange, BLACK, WHITE, rgb(0x0a0b0c), rgb(0x3daee9)] {
            assert_eq!(parse_hex(&format_hex(c)), Some(c));
        }
    }

    #[test]
    fn accent2_is_shifted_hue() {
        // От голубого Breeze — в сторону фиолетового, яркость та же.
        let a2 = shifted_hue(rgb(0x3daee9));
        assert!(a2.b() > a2.r() && a2.r() > a2.g(), "{a2:?}");
        assert_eq!(a2.b(), 0xe9);
        // У серого оттенка нет — второй цвет тот же.
        assert_eq!(shifted_hue(rgb(0x808080)), rgb(0x808080));
    }
}
