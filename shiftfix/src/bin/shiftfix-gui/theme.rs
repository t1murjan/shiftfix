//! Цвета, шрифты и стиль в духе KDE Breeze; тема следует системной (светлая/тёмная).

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Shadow, Stroke,
    TextStyle, Theme, ThemePreference, Visuals, vec2,
};

#[derive(Clone, Copy)]
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
    pub accent2: Color32,
    pub positive: Color32,
    pub warning: Color32,
    pub negative: Color32,
    pub hover: Color32,
}

pub const BOLD: &str = "bold";

pub fn palette(dark: bool) -> Palette {
    let accent = Color32::from_rgb(0x3d, 0xae, 0xe9);
    let accent2 = Color32::from_rgb(0x7c, 0x5c, 0xe0);
    if dark {
        Palette {
            dark,
            bg: Color32::from_rgb(0x1b, 0x1e, 0x20),
            sidebar: Color32::from_rgb(0x20, 0x23, 0x26),
            card: Color32::from_rgb(0x27, 0x2a, 0x2e),
            card_border: Color32::from_rgb(0x36, 0x3a, 0x3f),
            field: Color32::from_rgb(0x1f, 0x22, 0x25),
            text: Color32::from_rgb(0xfc, 0xfc, 0xfc),
            weak: Color32::from_rgb(0xa1, 0xa9, 0xb1),
            accent,
            accent2,
            positive: Color32::from_rgb(0x3d, 0xd4, 0x7f),
            warning: Color32::from_rgb(0xf6, 0x74, 0x00),
            negative: Color32::from_rgb(0xed, 0x54, 0x65),
            hover: Color32::from_rgb(0x30, 0x34, 0x38),
        }
    } else {
        Palette {
            dark,
            bg: Color32::from_rgb(0xf3, 0xf4, 0xf6),
            sidebar: Color32::from_rgb(0xe9, 0xeb, 0xee),
            card: Color32::WHITE,
            card_border: Color32::from_rgb(0xdc, 0xdf, 0xe3),
            field: Color32::from_rgb(0xf7, 0xf8, 0xf9),
            text: Color32::from_rgb(0x23, 0x26, 0x29),
            weak: Color32::from_rgb(0x6e, 0x76, 0x80),
            accent: Color32::from_rgb(0x1d, 0x99, 0xf3),
            accent2,
            positive: Color32::from_rgb(0x27, 0xae, 0x60),
            warning: Color32::from_rgb(0xd5, 0x6a, 0x00),
            negative: Color32::from_rgb(0xda, 0x44, 0x53),
            hover: Color32::from_rgb(0xdd, 0xe1, 0xe6),
        }
    }
}

pub fn current(ctx: &egui::Context) -> Palette {
    palette(ctx.theme() == Theme::Dark)
}

fn visuals(p: &Palette) -> Visuals {
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
    v.selection.bg_fill = p.accent.gamma_multiply(if p.dark { 0.55 } else { 0.35 });
    v.selection.stroke = Stroke::new(1.5, p.accent);
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
    let button = if p.dark {
        Color32::from_rgb(0x31, 0x35, 0x3a)
    } else {
        Color32::from_rgb(0xe8, 0xea, 0xed)
    };
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.card;
    w.noninteractive.weak_bg_fill = p.card;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.card_border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = radius;
    for (state, fill, stroke) in [
        (&mut w.inactive, button, p.card_border),
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeChoice {
    System,
    Light,
    Dark,
}

impl ThemeChoice {
    pub fn parse(s: &str) -> Option<ThemeChoice> {
        match s.trim() {
            "system" => Some(ThemeChoice::System),
            "light" => Some(ThemeChoice::Light),
            "dark" => Some(ThemeChoice::Dark),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            ThemeChoice::System => "system",
            ThemeChoice::Light => "light",
            ThemeChoice::Dark => "dark",
        }
    }
}

fn choice_file() -> std::path::PathBuf {
    shiftfix::config::Config::default_path().with_file_name("gui-theme")
}

/// Тема интерфейса, выбранная в окне (по умолчанию — как в системе).
pub fn saved_choice() -> ThemeChoice {
    std::fs::read_to_string(choice_file())
        .ok()
        .and_then(|s| ThemeChoice::parse(&s))
        .unwrap_or(ThemeChoice::System)
}

pub fn set_choice(ctx: &egui::Context, choice: ThemeChoice, persist: bool) {
    ctx.options_mut(|o| {
        o.theme_preference = match choice {
            ThemeChoice::System => ThemePreference::System,
            ThemeChoice::Light => ThemePreference::Light,
            ThemeChoice::Dark => ThemePreference::Dark,
        };
    });
    if persist {
        let path = choice_file();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, choice.as_str());
    }
}

pub fn apply(ctx: &egui::Context) {
    for theme in [Theme::Light, Theme::Dark] {
        let p = palette(theme == Theme::Dark);
        ctx.set_visuals_of(theme, visuals(&p));
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
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    if let Some(list) = fonts.families.get_mut(&FontFamily::Proportional) {
        list.retain(|name| name != "phosphor");
    }
    fonts
        .families
        .insert(FontFamily::Name("phosphor".into()), vec!["phosphor".into()]);

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
