//! Встроенные палитры: цвета окна для светлого и тёмного варианта.

use eframe::egui::Color32;

/// Цвета окна для одного варианта палитры (светлого или тёмного).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub dark: bool,
    pub bg: Color32,
    pub sidebar: Color32,
    pub card: Color32,
    pub card_border: Color32,
    pub field: Color32,
    pub text: Color32,
    pub weak: Color32,
    pub accent: Color32,
    /// Текст и иконки на заливке акцентом: чёрный или белый — что контрастнее.
    pub on_accent: Color32,
    pub positive: Color32,
    pub warning: Color32,
    pub negative: Color32,
    pub hover: Color32,
    /// Обычная (не основная) кнопка.
    pub button: Color32,
    /// Дорожка выключенного переключателя.
    pub toggle_off: Color32,
    /// Ручка переключателя.
    pub knob: Color32,
    /// Изображение клавиши: верх и край.
    pub key_face: Color32,
    pub key_edge: Color32,
    /// Точка активности устройства в покое.
    pub dot_idle: Color32,
}

/// Встроенная палитра: светлый и тёмный вариант.
pub struct PaletteDef {
    /// Имя в config.toml (`light_palette`, `dark_palette`).
    pub id: &'static str,
    /// Имя в окне.
    pub name: &'static str,
    pub light: Palette,
    pub dark: Palette,
}

pub(super) const fn rgb(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

pub(super) const BLACK: Color32 = Color32::BLACK;
pub(super) const WHITE: Color32 = Color32::WHITE;

/// Палитра KDE Breeze — оформление окна до 0.4.0.
const BREEZE: PaletteDef = PaletteDef {
    id: "breeze",
    name: "Breeze",
    light: Palette {
        dark: false,
        bg: rgb(0xf3f4f6),
        sidebar: rgb(0xe9ebee),
        card: WHITE,
        card_border: rgb(0xdcdfe3),
        field: rgb(0xf7f8f9),
        text: rgb(0x232629),
        weak: rgb(0x6e7680),
        accent: rgb(0x1d99f3),
        on_accent: BLACK,
        positive: rgb(0x27ae60),
        warning: rgb(0xd56a00),
        negative: rgb(0xda4453),
        hover: rgb(0xdde1e6),
        button: rgb(0xe8eaed),
        toggle_off: rgb(0xc5cad0),
        knob: WHITE,
        key_face: rgb(0xfbfbfc),
        key_edge: rgb(0xc5cad0),
        dot_idle: rgb(0xcfd3d8),
    },
    dark: Palette {
        dark: true,
        bg: rgb(0x1b1e20),
        sidebar: rgb(0x202326),
        card: rgb(0x272a2e),
        card_border: rgb(0x363a3f),
        field: rgb(0x1f2225),
        text: rgb(0xfcfcfc),
        weak: rgb(0xa1a9b1),
        accent: rgb(0x3daee9),
        on_accent: BLACK,
        positive: rgb(0x3dd47f),
        warning: rgb(0xf67400),
        negative: rgb(0xed5465),
        hover: rgb(0x303438),
        button: rgb(0x31353a),
        toggle_off: rgb(0x4a4f55),
        knob: WHITE,
        key_face: rgb(0x35393e),
        key_edge: rgb(0x16181a),
        dot_idle: rgb(0x454a50),
    },
};

/// Nord: Snow Storm и Polar Night, акцент — Frost.
const NORD: PaletteDef = PaletteDef {
    id: "nord",
    name: "Nord",
    light: Palette {
        dark: false,
        bg: rgb(0xe5e9f0),
        sidebar: rgb(0xd8dee9),
        card: rgb(0xf4f6f9),
        card_border: rgb(0xd8dee9),
        field: rgb(0xeceff4),
        text: rgb(0x2e3440),
        weak: rgb(0x5e6779),
        accent: rgb(0x5e81ac),
        on_accent: BLACK,
        positive: rgb(0x5f8a49),
        warning: rgb(0xb5633f),
        negative: rgb(0xbf616a),
        hover: rgb(0xcfd6e2),
        button: rgb(0xe2e7ee),
        toggle_off: rgb(0xb8c1cf),
        knob: WHITE,
        key_face: rgb(0xf9fafc),
        key_edge: rgb(0xc0c8d4),
        dot_idle: rgb(0xc8cfda),
    },
    dark: Palette {
        dark: true,
        bg: rgb(0x2e3440),
        sidebar: rgb(0x323946),
        card: rgb(0x3b4252),
        card_border: rgb(0x4c566a),
        field: rgb(0x353c4a),
        text: rgb(0xeceff4),
        weak: rgb(0xa3abb9),
        accent: rgb(0x88c0d0),
        on_accent: BLACK,
        positive: rgb(0xa3be8c),
        warning: rgb(0xebcb8b),
        negative: rgb(0xd57780),
        hover: rgb(0x434a5a),
        button: rgb(0x454d5f),
        toggle_off: rgb(0x616e88),
        knob: rgb(0xeceff4),
        key_face: rgb(0x464e60),
        key_edge: rgb(0x242933),
        dot_idle: rgb(0x566075),
    },
};

/// Catppuccin: Latte и Mocha, акцент — Mauve.
const CATPPUCCIN: PaletteDef = PaletteDef {
    id: "catppuccin",
    name: "Catppuccin",
    light: Palette {
        dark: false,
        bg: rgb(0xe6e9ef),
        sidebar: rgb(0xdce0e8),
        card: rgb(0xeff1f5),
        card_border: rgb(0xccd0da),
        field: rgb(0xe6e9ef),
        text: rgb(0x4c4f69),
        weak: rgb(0x6c6f85),
        accent: rgb(0x8839ef),
        on_accent: WHITE,
        positive: rgb(0x40a02b),
        warning: rgb(0xfe640b),
        negative: rgb(0xd20f39),
        hover: rgb(0xd4d8e1),
        button: rgb(0xdce0e8),
        toggle_off: rgb(0xbcc0cc),
        knob: WHITE,
        key_face: rgb(0xf7f8fa),
        key_edge: rgb(0xbcc0cc),
        dot_idle: rgb(0xc6cad5),
    },
    dark: Palette {
        dark: true,
        bg: rgb(0x1e1e2e),
        sidebar: rgb(0x181825),
        card: rgb(0x313244),
        card_border: rgb(0x45475a),
        field: rgb(0x1e1e2e),
        text: rgb(0xcdd6f4),
        weak: rgb(0xa6adc8),
        accent: rgb(0xcba6f7),
        on_accent: BLACK,
        positive: rgb(0xa6e3a1),
        warning: rgb(0xfab387),
        negative: rgb(0xf38ba8),
        hover: rgb(0x414356),
        button: rgb(0x45475a),
        toggle_off: rgb(0x585b70),
        knob: rgb(0xf5e0dc),
        key_face: rgb(0x45475a),
        key_edge: rgb(0x11111b),
        dot_idle: rgb(0x585b70),
    },
};

/// Gruvbox: светлый и тёмный, акцент — оранжевый.
const GRUVBOX: PaletteDef = PaletteDef {
    id: "gruvbox",
    name: "Gruvbox",
    light: Palette {
        dark: false,
        bg: rgb(0xf2e5bc),
        sidebar: rgb(0xebdbb2),
        card: rgb(0xfbf1c7),
        card_border: rgb(0xd5c4a1),
        field: rgb(0xf2e5bc),
        text: rgb(0x3c3836),
        weak: rgb(0x7c6f64),
        accent: rgb(0xd65d0e),
        on_accent: BLACK,
        positive: rgb(0x79740e),
        warning: rgb(0xb57614),
        negative: rgb(0x9d0006),
        hover: rgb(0xe2d1a6),
        button: rgb(0xebdbb2),
        toggle_off: rgb(0xbdae93),
        knob: rgb(0xfbf1c7),
        key_face: rgb(0xf9f5d7),
        key_edge: rgb(0xbdae93),
        dot_idle: rgb(0xd5c4a1),
    },
    dark: Palette {
        dark: true,
        bg: rgb(0x282828),
        sidebar: rgb(0x32302f),
        card: rgb(0x3c3836),
        card_border: rgb(0x504945),
        field: rgb(0x32302f),
        text: rgb(0xebdbb2),
        weak: rgb(0xa89984),
        accent: rgb(0xfe8019),
        on_accent: BLACK,
        positive: rgb(0xb8bb26),
        warning: rgb(0xfabd2f),
        negative: rgb(0xfb4934),
        hover: rgb(0x48433f),
        button: rgb(0x504945),
        toggle_off: rgb(0x665c54),
        knob: rgb(0xfbf1c7),
        key_face: rgb(0x504945),
        key_edge: rgb(0x1d2021),
        dot_idle: rgb(0x665c54),
    },
};

/// Встроенные палитры. Новая палитра — одна запись здесь: окно и тесты
/// перебирают этот список. Первая — запасная для неизвестного имени.
pub const PALETTES: &[PaletteDef] = &[BREEZE, NORD, CATPPUCCIN, GRUVBOX];

/// Палитра по имени из конфига; неизвестное имя — Breeze (о нём при запуске
/// окна предупреждает `unknown_palettes`).
pub fn find(id: &str) -> &'static PaletteDef {
    lookup(id).unwrap_or(&PALETTES[0])
}

/// Палитра по имени из конфига; `None` — такой нет.
pub fn lookup(id: &str) -> Option<&'static PaletteDef> {
    let id = id.trim();
    PALETTES.iter().find(|def| def.id.eq_ignore_ascii_case(id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::color::{
        ACCENT_MIN_CONTRAST, KNOB_MIN_CONTRAST, accent_readable, contrast, knob_on, on_accent,
    };

    /// Все варианты всех палитр: (имя для сообщения, палитра, должна ли быть тёмной).
    fn variants() -> Vec<(String, &'static Palette, bool)> {
        PALETTES
            .iter()
            .flat_map(|def| {
                [
                    (format!("{} light", def.id), &def.light, false),
                    (format!("{} dark", def.id), &def.dark, true),
                ]
            })
            .collect()
    }

    #[test]
    fn palette_ids() {
        for (i, def) in PALETTES.iter().enumerate() {
            assert!(!def.name.is_empty());
            assert_eq!(def.id, def.id.to_lowercase(), "{}", def.id);
            for other in &PALETTES[i + 1..] {
                assert_ne!(def.id, other.id);
            }
            assert_eq!(find(def.id).id, def.id);
        }
        assert_eq!(PALETTES[0].id, "breeze");
        assert_eq!(find("nope").id, "breeze");
        assert_eq!(find("").id, "breeze");
        assert_eq!(find(" Nord ").id, "nord");
    }

    #[test]
    fn palettes_readable() {
        for (name, p, dark) in variants() {
            assert_eq!(p.dark, dark, "{name}: флаг dark");
            for (what, fg, bg, min) in [
                ("text/bg", p.text, p.bg, 4.5),
                ("text/card", p.text, p.card, 4.5),
                ("text/field", p.text, p.field, 4.5),
                ("text/button", p.text, p.button, 4.5),
                ("weak/card", p.weak, p.card, 3.0),
                ("accent/card", p.accent, p.card, ACCENT_MIN_CONTRAST),
                ("knob/toggle_off", p.knob, p.toggle_off, KNOB_MIN_CONTRAST),
                ("knob/accent", p.knob, p.accent, KNOB_MIN_CONTRAST),
            ] {
                let c = contrast(fg, bg);
                assert!(c >= min, "{name}: {what} = {c:.2} < {min}");
            }
            assert_eq!(p.on_accent, on_accent(p.accent), "{name}: on_accent");
            // Предупреждение «акцент плохо виден» на своих палитрах не появляется.
            assert!(accent_readable(p), "{name}: accent_readable");
            assert_eq!(knob_on(p), p.knob, "{name}: ручка включённого");
        }
    }
}
