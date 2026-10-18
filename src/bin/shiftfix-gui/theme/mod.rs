//! Оформление окна: палитры, тема системы и стиль виджетов.
//!
//! Палитры — данные (`palettes.rs`); итоговые цвета для кадра даёт `resolve`:
//! режим (светлая, тёмная, как в системе), палитра варианта и акцент.

mod color;
mod legacy;
mod palettes;
mod state;
mod style;
mod system;
#[cfg(test)]
mod tests;

use std::time::Instant;

use shiftfix::config::{ThemeMode, ThemeSettings};

pub use color::{
    HexCommit, accent_readable, best_on, format_hex, hex_commit, knob_on, on_accent, parse_hex,
};
pub use legacy::migrate_legacy;
pub use palettes::{PALETTES, Palette, PaletteDef, find};
pub use state::ThemeState;
pub use style::{BOLD, apply_palette, apply_style, install_fonts};
pub use system::{System, SystemNote, SystemWatch, system_accent_blocked};

/// Вариант палитры: у каждого свои палитра и акцент в `[theme]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Variant {
    Light,
    Dark,
}

impl Variant {
    pub fn of(dark: bool) -> Variant {
        if dark { Variant::Dark } else { Variant::Light }
    }

    /// Цвета этого варианта во встроенной палитре.
    pub fn of_def(self, def: &PaletteDef) -> &Palette {
        match self {
            Variant::Light => &def.light,
            Variant::Dark => &def.dark,
        }
    }

    /// Имя палитры варианта (`light_palette` или `dark_palette`).
    pub fn palette_id(self, t: &ThemeSettings) -> &str {
        match self {
            Variant::Light => &t.light_palette,
            Variant::Dark => &t.dark_palette,
        }
    }

    pub fn palette_id_mut(self, t: &mut ThemeSettings) -> &mut String {
        match self {
            Variant::Light => &mut t.light_palette,
            Variant::Dark => &mut t.dark_palette,
        }
    }

    /// Свой акцент варианта: «#rrggbb» или пустая строка.
    pub fn accent(self, t: &ThemeSettings) -> &str {
        match self {
            Variant::Light => &t.light_accent,
            Variant::Dark => &t.dark_accent,
        }
    }

    pub fn accent_mut(self, t: &mut ThemeSettings) -> &mut String {
        match self {
            Variant::Light => &mut t.light_accent,
            Variant::Dark => &mut t.dark_accent,
        }
    }
}

/// Действует ли сейчас акцент системы: только в режиме «как в системе»,
/// с `system_accent` и если система его сообщает.
pub fn system_accent_active(t: &ThemeSettings, sys: &System, mode: ThemeMode) -> bool {
    mode == ThemeMode::System && t.system_accent && sys.available && sys.accent.is_some()
}

/// Итоговые цвета окна. `forced` — режим из `--theme`, он перекрывает `t.mode`.
///
/// Режим «как в системе» без портала или без предпочтения — тёмная тема.
pub fn resolve(t: &ThemeSettings, sys: &System, forced: Option<ThemeMode>) -> Palette {
    let mode = forced.unwrap_or(t.mode);
    let dark = match mode {
        ThemeMode::Light => false,
        ThemeMode::Dark => true,
        ThemeMode::System => !sys.available || sys.dark.unwrap_or(true),
    };
    variant_palette(t, sys, mode, Variant::of(dark))
}

/// Цвета варианта в режиме `mode` — и того, что сейчас не на экране.
/// Акцент: системный (см. `system_accent_active`), иначе свой «#rrggbb»,
/// иначе акцент палитры.
pub fn variant_palette(t: &ThemeSettings, sys: &System, mode: ThemeMode, v: Variant) -> Palette {
    let mut p = *v.of_def(find(v.palette_id(t)));
    let system_accent = sys.accent.filter(|_| system_accent_active(t, sys, mode));
    if let Some(accent) = system_accent.or_else(|| parse_hex(v.accent(t))) {
        p.accent = accent;
        p.accent2 = color::shifted_hue(accent);
    }
    p.on_accent = on_accent(p.accent);
    p
}

/// «Сбросить» в карточке «Оформление»: палитра и свой акцент варианта и акцент
/// системы — по умолчанию. Режим и второй вариант не меняются.
pub fn reset_variant(t: &mut ThemeSettings, v: Variant) {
    let defaults = ThemeSettings::default();
    *v.palette_id_mut(t) = v.palette_id(&defaults).to_owned();
    *v.accent_mut(t) = v.accent(&defaults).to_owned();
    t.system_accent = defaults.system_accent;
}

/// Включить или выключить свой акцент варианта. Включённый начинается с
/// акцента палитры: окно не меняется, пока цвет не выбран.
pub fn set_own_accent(t: &mut ThemeSettings, v: Variant, on: bool) {
    let accent = if on {
        format_hex(v.of_def(find(v.palette_id(t))).accent)
    } else {
        String::new()
    };
    let slot = v.accent_mut(t);
    if !on || slot.trim().is_empty() {
        *slot = accent;
    }
}

/// Пора ли записать тему: срок вышел и кнопка мыши отпущена. Пока тянут
/// ползунок пипетки, цвет меняется каждый кадр, а файл пишется один раз.
pub fn save_due(at: Option<Instant>, now: Instant, pointer_down: bool) -> bool {
    at.is_some_and(|at| now >= at) && !pointer_down
}
