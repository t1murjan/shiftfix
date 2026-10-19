//! Стиль окна в духе KDE Breeze: цвета виджетов egui из палитры, отступы и шрифты.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Shadow, Stroke,
    TextStyle, Theme, ThemePreference, Visuals, vec2,
};

use super::Palette;

/// Семейство жирного шрифта (заголовки).
pub const BOLD: &str = "bold";

/// Семейство шрифта с иконками Phosphor.
pub const ICONS: &str = "phosphor";

/// Стиль виджетов egui из палитры.
pub fn visuals(p: &Palette) -> Visuals {
    let mut v = if p.dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    v.panel_fill = p.bg;
    v.window_fill = p.card;
    v.window_stroke = Stroke::new(1.0, p.card_border);
    v.extreme_bg_color = p.field;
    v.text_edit_bg_color = Some(p.field);
    v.faint_bg_color = p.hover;
    v.code_bg_color = p.field;
    v.hyperlink_color = p.accent;
    v.warn_fg_color = p.warning;
    v.error_fg_color = p.negative;
    v.override_text_color = None;
    // Рамка поля в фокусе и выделенный текст — акцентом, как раньше.
    v.selection.bg_fill = p.accent.gamma_multiply(if p.dark { 0.55 } else { 0.35 });
    v.selection.stroke = Stroke::new(1.5, p.accent);
    // Иначе курсор остаётся синим от egui на любой палитре.
    v.text_cursor.stroke.color = p.text;
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = CornerRadius::same(10);
    let shadow = Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha(if p.dark { 110 } else { 40 }),
    };
    v.window_shadow = shadow;
    v.popup_shadow = shadow;
    v.slider_trailing_fill = true;

    let radius = CornerRadius::same(8);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.card;
    w.noninteractive.weak_bg_fill = p.card;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.card_border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = radius;
    for (state, fill, stroke) in [
        (&mut w.inactive, p.button, p.card_border),
        (&mut w.hovered, p.hover, p.accent.gamma_multiply(0.7)),
        (&mut w.active, p.accent.gamma_multiply(0.35), p.accent),
        (&mut w.open, p.hover, p.accent),
    ] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.bg_stroke = Stroke::new(1.0, stroke);
        state.fg_stroke = Stroke::new(1.5, p.text);
        state.corner_radius = radius;
        state.expansion = 0.0;
    }
    v
}

/// Отступы и шрифты — один раз, для стилей обеих тем egui.
pub fn apply_style(ctx: &egui::Context) {
    for theme in [Theme::Light, Theme::Dark] {
        ctx.style_mut_of(theme, |style| {
            style.spacing.item_spacing = vec2(10.0, 8.0);
            style.spacing.button_padding = vec2(14.0, 6.0);
            style.spacing.interact_size.y = 30.0;
            style.spacing.slider_width = 260.0;
            style.spacing.combo_width = 220.0;
            style.text_styles = [
                (
                    TextStyle::Small,
                    FontId::new(12.5, FontFamily::Proportional),
                ),
                (TextStyle::Body, FontId::new(15.0, FontFamily::Proportional)),
                (
                    TextStyle::Button,
                    FontId::new(15.0, FontFamily::Proportional),
                ),
                (
                    TextStyle::Heading,
                    FontId::new(24.0, FontFamily::Name(BOLD.into())),
                ),
                (
                    TextStyle::Monospace,
                    FontId::new(13.5, FontFamily::Monospace),
                ),
            ]
            .into();
        });
    }
}

/// Цвета палитры для виджетов egui. Режим ставится явно: `ThemePreference::System`
/// на Linux всегда тёмный (winit 0.30 не сообщает тему системы).
pub fn apply_palette(ctx: &egui::Context, p: &Palette) {
    let v = visuals(p);
    ctx.set_visuals_of(Theme::Light, v.clone());
    ctx.set_visuals_of(Theme::Dark, v);
    ctx.options_mut(|o| {
        o.theme_preference = if p.dark {
            ThemePreference::Dark
        } else {
            ThemePreference::Light
        };
    });
}

/// Файл шрифта, который fontconfig подбирает по шаблону.
fn system_font(pattern: &str) -> Option<Vec<u8>> {
    let out = Command::new("fc-match")
        .args(["-f", "%{file}", pattern])
        .output()
        .ok()?;
    let path = String::from_utf8(out.stdout).ok()?;
    let ext = Path::new(&path).extension()?.to_str()?.to_lowercase();
    if !matches!(ext.as_str(), "ttf" | "otf") {
        return None;
    }
    std::fs::read(path).ok()
}

/// Системный шрифт интерфейса (Noto Sans в Plasma), иконки Phosphor и
/// встроенные шрифты egui как запасные.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let mut put = |name: &str, family: FontFamily, bytes: Vec<u8>| {
        fonts
            .font_data
            .insert(name.into(), Arc::new(FontData::from_owned(bytes)));
        fonts
            .families
            .entry(family)
            .or_default()
            .insert(0, name.into());
    };
    if let Some(bytes) = system_font("sans-serif:style=Regular") {
        put("system", FontFamily::Proportional, bytes);
    }
    if let Some(bytes) = system_font("monospace:style=Regular") {
        put("system-mono", FontFamily::Monospace, bytes);
    }
    // Иконки Phosphor — отдельным семейством: у части шрифтов (Inter и др.) есть свои
    // глифы в той же области Private Use, а у Phosphor — пустые латинские буквы.
    // "phosphor" ниже — имя данных шрифта, под которым его кладёт egui_phosphor.
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    if let Some(list) = fonts.families.get_mut(&FontFamily::Proportional) {
        list.retain(|name| name != "phosphor");
    }
    fonts
        .families
        .insert(FontFamily::Name(ICONS.into()), vec!["phosphor".into()]);

    let mut bold = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    if let Some(bytes) = system_font("sans-serif:style=Bold") {
        fonts
            .font_data
            .insert("system-bold".into(), Arc::new(FontData::from_owned(bytes)));
        bold.insert(0, "system-bold".into());
    }
    fonts.families.insert(FontFamily::Name(BOLD.into()), bold);
    ctx.set_fonts(fonts);
}
