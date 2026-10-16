//! Палитры, вычисление темы и стиль окна в духе KDE Breeze.
//!
//! Палитры — данные (`PALETTES`); итоговые цвета для кадра даёт `resolve`:
//! режим (светлая, тёмная, как в системе), палитра варианта и акцент.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Shadow, Stroke,
    TextStyle, Theme, ThemePreference, Visuals, ecolor, vec2,
};
use shiftfix::config::{ThemeMode, ThemeSettings};
use shiftfix::portal::{self, PortalState, Scheme};

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
    pub accent2: Color32,
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
    /// Градиент заголовка «Главной»: служба на паузе.
    pub paused_from: Color32,
    pub paused_to: Color32,
    /// Градиент заголовка «Главной»: служба остановлена или не установлена.
    pub stopped_from: Color32,
    pub stopped_to: Color32,
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

pub const BOLD: &str = "bold";

const fn rgb(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

const BLACK: Color32 = Color32::BLACK;
const WHITE: Color32 = Color32::WHITE;

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
        accent2: rgb(0x7c5ce0),
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
        paused_from: rgb(0x8a939d),
        paused_to: rgb(0x5f6772),
        stopped_from: rgb(0xe06b5b),
        stopped_to: rgb(0xb84a8f),
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
        accent2: rgb(0x7c5ce0),
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
        paused_from: rgb(0x8a939d),
        paused_to: rgb(0x5f6772),
        stopped_from: rgb(0xe06b5b),
        stopped_to: rgb(0xb84a8f),
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
        accent2: rgb(0xb48ead),
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
        paused_from: rgb(0x7b88a1),
        paused_to: rgb(0x4c566a),
        stopped_from: rgb(0xd08770),
        stopped_to: rgb(0xbf616a),
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
        accent2: rgb(0xb48ead),
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
        paused_from: rgb(0x7b88a1),
        paused_to: rgb(0x4c566a),
        stopped_from: rgb(0xd08770),
        stopped_to: rgb(0xbf616a),
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
        accent2: rgb(0x1e66f5),
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
        paused_from: rgb(0x8c8fa1),
        paused_to: rgb(0x5c5f77),
        stopped_from: rgb(0xd20f39),
        stopped_to: rgb(0xfe640b),
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
        accent2: rgb(0x89b4fa),
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
        paused_from: rgb(0x7f849c),
        paused_to: rgb(0x585b70),
        stopped_from: rgb(0xf38ba8),
        stopped_to: rgb(0xfab387),
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
        accent2: rgb(0xd79921),
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
        paused_from: rgb(0xa89984),
        paused_to: rgb(0x7c6f64),
        stopped_from: rgb(0xcc241d),
        stopped_to: rgb(0xb16286),
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
        accent2: rgb(0xfabd2f),
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
        paused_from: rgb(0x928374),
        paused_to: rgb(0x665c54),
        stopped_from: rgb(0xfb4934),
        stopped_to: rgb(0xd3869b),
    },
};

/// Встроенные палитры. Новая палитра — одна запись здесь: окно и тесты
/// перебирают этот список. Первая — запасная для неизвестного имени.
pub const PALETTES: &[PaletteDef] = &[BREEZE, NORD, CATPPUCCIN, GRUVBOX];

/// Палитра по имени из конфига; неизвестное имя — Breeze.
pub fn find(id: &str) -> &'static PaletteDef {
    let id = id.trim();
    PALETTES
        .iter()
        .find(|def| def.id.eq_ignore_ascii_case(id))
        .unwrap_or(&PALETTES[0])
}

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

/// Что сделать с полем «#rrggbb», когда оно потеряло фокус.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HexCommit {
    /// В поле ничего не набирали — принимать нечего.
    Keep,
    /// Набранное отменено — забыть его.
    Discard,
    /// Принять цвет.
    Apply(Color32),
    /// Не цвет — оставить набранное и подсветить.
    Bad,
}

/// Уход из поля hex. Принимается только набранное (`edited`): если цвет
/// сменили кнопкой («Сбросить», «Свой акцент», пипетка), набранное уже
/// забыто, в поле сохранённый цвет, и уход из поля не должен его вернуть.
/// Esc, пустое поле или поле, ставшее неактивным, — отмена.
pub fn hex_commit(edited: bool, enabled: bool, escape: bool, text: &str) -> HexCommit {
    if !edited {
        HexCommit::Keep
    } else if escape || !enabled || text.trim().is_empty() {
        HexCommit::Discard
    } else {
        parse_hex(text).map_or(HexCommit::Bad, HexCommit::Apply)
    }
}

/// Сдвиг оттенка второго цвета градиента от своего акцента: как у Breeze,
/// где после голубого идёт фиолетовый (около 54°).
const ACCENT2_HUE_SHIFT: f32 = 0.15;

/// Второй цвет градиента для своего или системного акцента: тот же цвет
/// (HSV в гамма-пространстве) со сдвигом оттенка.
fn shifted_hue(c: Color32) -> Color32 {
    let (h, s, v) = ecolor::hsv_from_rgb([c.r(), c.g(), c.b()].map(|x| f32::from(x) / 255.0));
    let [r, g, b] = ecolor::rgb_from_hsv(((h + ACCENT2_HUE_SHIFT).fract(), s, v));
    let u8_of = |x: f32| (x * 255.0).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(u8_of(r), u8_of(g), u8_of(b))
}

/// Что сообщила система (XDG Desktop Portal). По умолчанию — портала нет.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct System {
    /// Портал ответил.
    pub available: bool,
    /// color-scheme: `Some(true)` — тёмная, `Some(false)` — светлая,
    /// `None` — предпочтения нет.
    pub dark: Option<bool>,
    /// accent-color, если система его сообщает.
    pub accent: Option<Color32>,
}

/// Пока портал не ответил (или недоступен), цвета — как без портала:
/// тёмная тема и свой акцент. Чем это состояние отличается от «недоступен»,
/// окно смотрит в самом `PortalState`.
impl From<&PortalState> for System {
    fn from(state: &PortalState) -> System {
        let PortalState::Ready(a) = state else {
            return System::default();
        };
        System {
            available: true,
            dark: a.scheme.and_then(|scheme| match scheme {
                Scheme::Dark => Some(true),
                Scheme::Light => Some(false),
                Scheme::NoPreference => None,
            }),
            accent: a.accent.map(|[r, g, b]| Color32::from_rgb(r, g, b)),
        }
    }
}

/// Сколько окно при запуске ждёт первый ответ портала, чтобы не мигнуть
/// тёмной темой. Дольше — открываемся тёмными, тема сменится на лету.
const FIRST_ANSWER: Duration = Duration::from_millis(250);

/// Тема системы из портала: поток портала пишет, кадр читает.
pub struct SystemWatch {
    shared: Arc<Shared>,
    started: Instant,
}

#[derive(Default)]
struct Shared {
    state: Mutex<PortalState>,
    answered: Condvar,
    /// Окно, которое надо перерисовать при смене темы системы.
    ctx: OnceLock<egui::Context>,
}

fn lock(state: &Mutex<PortalState>) -> MutexGuard<'_, PortalState> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

impl SystemWatch {
    /// Начать слушать портал (сессионная шина) — ещё до создания окна.
    pub fn start() -> SystemWatch {
        let shared = Arc::new(Shared::default());
        let writer = Arc::clone(&shared);
        let spawned = portal::spawn(None, move |state| {
            *lock(&writer.state) = state;
            writer.answered.notify_all();
            if let Some(ctx) = writer.ctx.get() {
                ctx.request_repaint();
            }
        });
        if let Err(err) = spawned {
            log::warn!("портал настроек: поток не запущен: {err} — тёмная тема");
            *lock(&shared.state) = PortalState::Unavailable(err.to_string());
        }
        SystemWatch {
            shared,
            started: Instant::now(),
        }
    }

    /// Перерисовывать окно, когда сменится тема системы. Первый ответ
    /// подождать, но не дольше `FIRST_ANSWER` с запуска.
    pub fn attach(&self, ctx: &egui::Context) {
        let _ = self.shared.ctx.set(ctx.clone());
        let left = FIRST_ANSWER.saturating_sub(self.started.elapsed());
        let guard = lock(&self.shared.state);
        let _ = self
            .shared
            .answered
            .wait_timeout_while(guard, left, |s| *s == PortalState::Pending);
    }

    /// Что портал сообщил на сейчас.
    pub fn state(&self) -> PortalState {
        lock(&self.shared.state).clone()
    }
}

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
        p.accent2 = shifted_hue(accent);
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

/// Ниже этого контраста с фоном карточек акцентные значки, заголовки и
/// ссылки читаются плохо. Все встроенные палитры порог проходят (тест):
/// у Breeze light акцент к белой карточке — около 3.04.
pub const ACCENT_MIN_CONTRAST: f32 = 3.0;

/// Акцент виден на фоне карточек.
pub fn accent_readable(p: &Palette) -> bool {
    contrast(p.accent, p.card) >= ACCENT_MIN_CONTRAST
}

/// Ручка, слишком похожая на дорожку, теряется.
const KNOB_MIN_CONTRAST: f32 = 1.5;

/// Ручка включённого переключателя: цвет палитры, а на похожем на него
/// своём акценте — чёрная или белая.
pub fn knob_on(p: &Palette) -> Color32 {
    if contrast(p.knob, p.accent) >= KNOB_MIN_CONTRAST {
        p.knob
    } else {
        p.on_accent
    }
}

/// Что окно знает о теме системы — строка в карточке «Оформление».
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemNote {
    /// Портал ещё не ответил.
    Waiting,
    /// Портала нет: тёмная.
    Unavailable,
    /// Портал есть, но тема в системе не выбрана: тёмная.
    NoPreference,
    Dark,
    Light,
}

impl SystemNote {
    pub fn of(portal: &PortalState) -> SystemNote {
        match portal {
            PortalState::Pending => SystemNote::Waiting,
            PortalState::Unavailable(_) => SystemNote::Unavailable,
            PortalState::Ready(a) => match a.scheme {
                Some(Scheme::Dark) => SystemNote::Dark,
                Some(Scheme::Light) => SystemNote::Light,
                Some(Scheme::NoPreference) | None => SystemNote::NoPreference,
            },
        }
    }

    pub fn text(self) -> &'static str {
        match self {
            SystemNote::Waiting => "Ждём ответа системы…",
            SystemNote::Unavailable => "Система не сообщает тему — используется тёмная.",
            SystemNote::NoPreference => "В системе тема не выбрана — используется тёмная.",
            SystemNote::Dark => "Сейчас в системе тёмная тема.",
            SystemNote::Light => "Сейчас в системе светлая тема.",
        }
    }

    /// Тема выбрана не системой, а запасная.
    pub fn fallback(self) -> bool {
        matches!(self, SystemNote::Unavailable | SystemNote::NoPreference)
    }
}

/// Почему «Акцент как в системе» нельзя включить; `None` — можно. Пока портал
/// не ответил, переключатель не выключается.
pub fn system_accent_blocked(portal: &PortalState) -> Option<&'static str> {
    match portal {
        PortalState::Pending => None,
        PortalState::Unavailable(_) => Some("Система не сообщает тему и акцент."),
        PortalState::Ready(a) if a.accent.is_none() => {
            Some("Система не сообщает свой акцент (accent-color).")
        }
        PortalState::Ready(_) => None,
    }
}

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

/// Режим из параметра `--theme` (только на этот запуск, в файл не пишется).
pub fn parse_mode(s: &str) -> Option<ThemeMode> {
    match s.trim() {
        "system" => Some(ThemeMode::System),
        "light" => Some(ThemeMode::Light),
        "dark" => Some(ThemeMode::Dark),
        _ => None,
    }
}

/// Пора ли записать тему: срок вышел и кнопка мыши отпущена. Пока тянут
/// ползунок пипетки, цвет меняется каждый кадр, а файл пишется один раз.
pub fn save_due(at: Option<Instant>, now: Instant, pointer_down: bool) -> bool {
    at.is_some_and(|at| now >= at) && !pointer_down
}

/// Что делать со старым файлом `gui-theme` (режим окна до версии 0.4).
#[derive(Debug, PartialEq, Eq)]
pub enum Migration {
    /// Файла нет.
    Nothing,
    /// Перенести режим в `[theme]`, после удачной записи удалить файл.
    Adopt(ThemeMode),
    /// Режим в конфиге уже есть (или в файле мусор): файл просто удалить.
    Remove,
    /// Конфиг не читается — ничего не трогать, чтобы не потерять выбор.
    Keep,
}

pub fn migration(legacy: Option<&str>, config_ok: bool, config_has_mode: bool) -> Migration {
    let Some(text) = legacy else {
        return Migration::Nothing;
    };
    if !config_ok {
        return Migration::Keep;
    }
    match parse_mode(text) {
        Some(mode) if !config_has_mode => Migration::Adopt(mode),
        _ => Migration::Remove,
    }
}

/// Старый файл режима рядом с конфигом.
pub fn legacy_file(config: &Path) -> PathBuf {
    config.with_file_name("gui-theme")
}

/// Один раз перенести режим из `gui-theme` в `[theme]`. Ошибки — в журнал:
/// окно работает и без переноса.
pub fn migrate_legacy(config: &Path, theme: &mut ThemeSettings) {
    let legacy = legacy_file(config);
    let Ok(text) = fs::read_to_string(&legacy) else {
        return;
    };
    let raw = match fs::read_to_string(config) {
        Ok(text) => Some(text),
        Err(err) if err.kind() == ErrorKind::NotFound => None,
        Err(err) => {
            log::warn!("gui-theme не перенесён: {}: {err}", config.display());
            return;
        }
    };
    let table = raw.as_deref().map(str::parse::<toml::Table>);
    let config_ok = table.as_ref().is_none_or(Result::is_ok);
    let has_mode = table
        .and_then(Result::ok)
        .is_some_and(|t| t.get("theme").and_then(|v| v.get("mode")).is_some());
    let remove = || {
        if let Err(err) = fs::remove_file(&legacy) {
            log::warn!("не удалён {}: {err}", legacy.display());
        }
    };
    match migration(Some(&text), config_ok, has_mode) {
        Migration::Adopt(mode) => {
            let moved = ThemeSettings {
                mode,
                ..theme.clone()
            };
            match moved.save(config) {
                Ok(()) => {
                    *theme = moved;
                    remove();
                }
                Err(err) => log::warn!("gui-theme не перенесён: {err:#}"),
            }
        }
        Migration::Remove => remove(),
        Migration::Keep | Migration::Nothing => {}
    }
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn mode_from_argument() {
        assert_eq!(parse_mode("light"), Some(ThemeMode::Light));
        assert_eq!(parse_mode(" dark\n"), Some(ThemeMode::Dark));
        assert_eq!(parse_mode("system"), Some(ThemeMode::System));
        assert_eq!(parse_mode("Dark"), None);
    }

    #[test]
    fn save_waits_for_deadline_and_release() {
        let now = Instant::now();
        let later = now + std::time::Duration::from_millis(400);
        assert!(!save_due(None, later, false), "сохранять нечего");
        assert!(!save_due(Some(later), now, false), "срок не вышел");
        assert!(!save_due(Some(now), later, true), "ползунок ещё тянут");
        assert!(save_due(Some(now), later, false));
        assert!(save_due(Some(now), now, false), "ровно в срок");
    }

    #[test]
    fn legacy_migration_rules() {
        assert_eq!(migration(None, true, false), Migration::Nothing);
        assert_eq!(
            migration(Some("light\n"), true, false),
            Migration::Adopt(ThemeMode::Light)
        );
        assert_eq!(migration(Some("dark"), true, true), Migration::Remove);
        assert_eq!(migration(Some("мусор"), true, false), Migration::Remove);
        assert_eq!(migration(Some("light"), false, false), Migration::Keep);
    }

    #[test]
    fn legacy_file_moves_into_config() {
        let dir = std::env::temp_dir().join(format!("shiftfix-legacy-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let config = dir.join("config.toml");
        fs::write(&config, "# моя настройка\ndouble_tap_ms = 350\n").unwrap();
        fs::write(legacy_file(&config), "light").unwrap();

        let mut theme = ThemeSettings::default();
        migrate_legacy(&config, &mut theme);
        assert_eq!(theme.mode, ThemeMode::Light);
        assert!(!legacy_file(&config).exists(), "файл удалён после записи");
        assert_eq!(ThemeSettings::load(&config).unwrap().mode, ThemeMode::Light);
        let text = fs::read_to_string(&config).unwrap();
        assert!(text.starts_with("# моя настройка\ndouble_tap_ms = 350\n"));

        // Режим в конфиге уже есть — файл просто удаляется.
        fs::write(legacy_file(&config), "dark").unwrap();
        migrate_legacy(&config, &mut theme);
        assert_eq!(theme.mode, ThemeMode::Light);
        assert!(!legacy_file(&config).exists());

        // Битый конфиг не трогаем, файл остаётся.
        fs::write(&config, "double_tap_ms = [\n").unwrap();
        fs::write(legacy_file(&config), "dark").unwrap();
        migrate_legacy(&config, &mut theme);
        assert!(legacy_file(&config).exists());
        assert_eq!(fs::read_to_string(&config).unwrap(), "double_tap_ms = [\n");
        fs::remove_dir_all(dir).unwrap();
    }

    fn own_accent(mode: ThemeMode, light: &str, dark: &str) -> ThemeSettings {
        ThemeSettings {
            mode,
            light_accent: light.into(),
            dark_accent: dark.into(),
            ..ThemeSettings::default()
        }
    }

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

    #[test]
    fn accent_warning_threshold() {
        let light = |accent: &str| ThemeSettings {
            mode: ThemeMode::Light,
            light_accent: accent.into(),
            ..ThemeSettings::default()
        };
        let none = System::default();
        // Жёлтый, оранжевый и светло-серый на белой карточке видны плохо.
        for bad in ["#ffff00", "#ff8800", "#cccccc", "#ffffff"] {
            assert!(
                !accent_readable(&resolve(&light(bad), &none, None)),
                "{bad}"
            );
        }
        for good in ["#1d99f3", "#d65d0e", "#000000"] {
            assert!(
                accent_readable(&resolve(&light(good), &none, None)),
                "{good}"
            );
        }
        // На тёмной карточке — наоборот: тёмно-синий не виден.
        let dark = ThemeSettings {
            mode: ThemeMode::Dark,
            dark_accent: "#1e3a8a".into(),
            ..ThemeSettings::default()
        };
        assert!(!accent_readable(&resolve(&dark, &none, None)));
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
    fn variant_fields() {
        let mut t = ThemeSettings {
            light_palette: "nord".into(),
            dark_palette: "gruvbox".into(),
            light_accent: "#ff8800".into(),
            ..ThemeSettings::default()
        };
        assert_eq!(Variant::of(true), Variant::Dark);
        assert_eq!(Variant::of(false), Variant::Light);
        assert_eq!(Variant::Light.palette_id(&t), "nord");
        assert_eq!(Variant::Dark.palette_id(&t), "gruvbox");
        assert_eq!(Variant::Light.accent(&t), "#ff8800");
        assert_eq!(Variant::Dark.accent(&t), "");
        *Variant::Dark.accent_mut(&mut t) = "#000000".into();
        *Variant::Dark.palette_id_mut(&mut t) = "catppuccin".into();
        assert_eq!(t.dark_accent, "#000000");
        assert_eq!(t.dark_palette, "catppuccin");
        assert_eq!(t.light_palette, "nord");
        let def = find("nord");
        assert_eq!(Variant::Light.of_def(def), &def.light);
        assert_eq!(Variant::Dark.of_def(def), &def.dark);
    }

    #[test]
    fn variant_palette_matches_resolve() {
        let t = ThemeSettings {
            mode: ThemeMode::System,
            light_palette: "catppuccin".into(),
            dark_palette: "nord".into(),
            dark_accent: "#ff8800".into(),
            system_accent: true,
            ..ThemeSettings::default()
        };
        let sys = System {
            available: true,
            dark: Some(false),
            accent: Some(rgb(0x00aa55)),
        };
        // На экране светлый вариант, а тёмный можно посмотреть тут же.
        let on_screen = resolve(&t, &sys, None);
        assert_eq!(
            variant_palette(&t, &sys, ThemeMode::System, Variant::Light),
            on_screen
        );
        let dark = variant_palette(&t, &sys, ThemeMode::System, Variant::Dark);
        assert!(dark.dark);
        assert_eq!(dark.bg, find("nord").dark.bg);
        // В режиме «как в системе» акцент системы действует на оба варианта.
        assert_eq!(dark.accent, rgb(0x00aa55));
        assert!(system_accent_active(&t, &sys, ThemeMode::System));
        // В явном режиме — свой акцент.
        let dark = variant_palette(&t, &sys, ThemeMode::Dark, Variant::Dark);
        assert_eq!(dark.accent, rgb(0xff8800));
        assert!(!system_accent_active(&t, &sys, ThemeMode::Dark));
        let quiet = System {
            accent: None,
            ..sys
        };
        assert!(!system_accent_active(&t, &quiet, ThemeMode::System));
    }

    #[test]
    fn reset_variant_keeps_mode_and_other_variant() {
        let edited = ThemeSettings {
            mode: ThemeMode::Light,
            light_palette: "gruvbox".into(),
            dark_palette: "nord".into(),
            light_accent: "#ff8800".into(),
            dark_accent: "#00aa55".into(),
            system_accent: true,
        };
        let mut t = edited.clone();
        reset_variant(&mut t, Variant::Light);
        assert_eq!(
            t,
            ThemeSettings {
                light_palette: "breeze".into(),
                light_accent: String::new(),
                system_accent: false,
                ..edited.clone()
            }
        );
        let mut t = edited.clone();
        reset_variant(&mut t, Variant::Dark);
        assert_eq!(t.mode, ThemeMode::Light);
        assert_eq!(t.light_palette, "gruvbox");
        assert_eq!(t.light_accent, "#ff8800");
        assert_eq!(t.dark_palette, "breeze");
        assert_eq!(t.dark_accent, "");
        assert!(!t.system_accent);
        // Повторный сброс ничего не меняет.
        let once = t.clone();
        reset_variant(&mut t, Variant::Dark);
        assert_eq!(t, once);
        // Сброс всех вариантов — значения по умолчанию, кроме режима.
        reset_variant(&mut t, Variant::Light);
        assert_eq!(
            t,
            ThemeSettings {
                mode: ThemeMode::Light,
                ..ThemeSettings::default()
            }
        );
    }

    #[test]
    fn own_accent_switch() {
        let mut t = ThemeSettings {
            dark_palette: "gruvbox".into(),
            ..ThemeSettings::default()
        };
        // Включили — акцент палитры, окно не меняется.
        set_own_accent(&mut t, Variant::Dark, true);
        assert_eq!(t.dark_accent, format_hex(find("gruvbox").dark.accent));
        assert_eq!(t.light_accent, "");
        assert_eq!(
            resolve(&t, &System::default(), Some(ThemeMode::Dark)).accent,
            find("gruvbox").dark.accent
        );
        // Уже выбранный цвет повторное включение не затирает.
        t.dark_accent = "#ff8800".into();
        set_own_accent(&mut t, Variant::Dark, true);
        assert_eq!(t.dark_accent, "#ff8800");
        set_own_accent(&mut t, Variant::Dark, false);
        assert_eq!(t.dark_accent, "");
        // Пробелы — это «выключен».
        t.light_accent = "  ".into();
        set_own_accent(&mut t, Variant::Light, true);
        assert_eq!(t.light_accent, format_hex(find("breeze").light.accent));
    }

    #[test]
    fn system_notes() {
        use shiftfix::portal::Appearance;
        let ready = |scheme, accent| PortalState::Ready(Appearance { scheme, accent });
        let cases = [
            (PortalState::Pending, SystemNote::Waiting, None),
            (
                PortalState::Unavailable("нет шины".into()),
                SystemNote::Unavailable,
                Some(()),
            ),
            (ready(None, None), SystemNote::NoPreference, Some(())),
            (
                ready(Some(Scheme::NoPreference), Some([1, 2, 3])),
                SystemNote::NoPreference,
                None,
            ),
            (ready(Some(Scheme::Dark), None), SystemNote::Dark, Some(())),
            (
                ready(Some(Scheme::Light), Some([1, 2, 3])),
                SystemNote::Light,
                None,
            ),
        ];
        for (state, note, blocked) in cases {
            assert_eq!(SystemNote::of(&state), note, "{state:?}");
            assert_eq!(
                system_accent_blocked(&state).map(|_| ()),
                blocked,
                "{state:?}"
            );
        }
        // Запасная тёмная — только когда система тему не сообщила.
        assert!(SystemNote::Unavailable.fallback());
        assert!(SystemNote::NoPreference.fallback());
        for note in [SystemNote::Waiting, SystemNote::Dark, SystemNote::Light] {
            assert!(!note.fallback(), "{note:?}");
        }
        assert!(SystemNote::Unavailable.text().contains("тёмная"));
    }

    #[test]
    fn breeze_as_before() {
        // Breeze — цвета окна 0.3.0.
        let b = find("breeze");
        assert_eq!(b.dark.bg, rgb(0x1b1e20));
        assert_eq!(b.dark.accent, rgb(0x3daee9));
        assert_eq!(b.dark.button, rgb(0x31353a));
        assert_eq!(b.light.card, WHITE);
        assert_eq!(b.light.accent, rgb(0x1d99f3));
        assert_eq!(b.light.accent2, rgb(0x7c5ce0));
        assert_eq!(b.light.button, rgb(0xe8eaed));
        let p = resolve(&ThemeSettings::default(), &System::default(), None);
        assert_eq!(p, b.dark);
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
    fn hex_commit_only_typed() {
        use HexCommit::*;
        // «Сбросить» или выключенный «Свой акцент»: набранное забыто, в поле
        // акцент палитры — уход из поля его не записывает.
        assert_eq!(hex_commit(false, true, false, "#1d99f3"), Keep);
        assert_eq!(hex_commit(false, false, false, "#1d99f3"), Keep);
        assert_eq!(hex_commit(false, true, true, "#1d99f3"), Keep);
        // Набрали и ушли из поля (Enter, клик мимо) — принять.
        assert_eq!(
            hex_commit(true, true, false, "#00aa55"),
            Apply(rgb(0x00aa55))
        );
        assert_eq!(hex_commit(true, true, false, "#0a5"), Bad);
        // Esc, пусто или поле стало неактивным («Акцент как в системе»).
        assert_eq!(hex_commit(true, true, true, "#00aa55"), Discard);
        assert_eq!(hex_commit(true, true, false, "  "), Discard);
        assert_eq!(hex_commit(true, false, false, "#00aa55"), Discard);
    }

    #[test]
    fn resolve_own_accent() {
        let breeze = find("breeze");
        let t = own_accent(ThemeMode::Light, "#ff8800", "");
        let p = resolve(&t, &System::default(), None);
        assert!(!p.dark);
        assert_eq!(p.accent, rgb(0xff8800));
        assert_ne!(p.accent2, p.accent);
        assert_ne!(p.accent2, breeze.light.accent2);
        assert_eq!(p.on_accent, on_accent(p.accent));
        assert_eq!(p.bg, breeze.light.bg);
        // Акцент тёмного варианта светлый не трогает.
        let t = own_accent(ThemeMode::Light, "", "#ff8800");
        assert_eq!(resolve(&t, &System::default(), None), breeze.light);
        let p = resolve(&t, &System::default(), Some(ThemeMode::Dark));
        assert_eq!(p.accent, rgb(0xff8800));
        // Тёмный акцент — белый текст на кнопках.
        let t = own_accent(ThemeMode::Dark, "", "#1e3a8a");
        assert_eq!(resolve(&t, &System::default(), None).on_accent, WHITE);
    }

    #[test]
    fn resolve_bad_hex_keeps_palette_accent() {
        for bad in ["#f80", "#ff880080", "orange", "#gg0000", " "] {
            let t = ThemeSettings {
                mode: ThemeMode::Dark,
                dark_palette: "nord".into(),
                dark_accent: bad.into(),
                ..ThemeSettings::default()
            };
            assert_eq!(
                resolve(&t, &System::default(), None),
                find("nord").dark,
                "{bad:?}"
            );
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

    #[test]
    fn system_accent_only_in_system_mode() {
        let green = rgb(0x00aa55);
        let sys = System {
            available: true,
            dark: Some(true),
            accent: Some(green),
        };
        let mut t = own_accent(ThemeMode::System, "", "#ff8800");
        t.system_accent = true;
        assert_eq!(resolve(&t, &sys, None).accent, green);
        // Режим задан явно — свой акцент.
        t.mode = ThemeMode::Dark;
        assert_eq!(resolve(&t, &sys, None).accent, rgb(0xff8800));
        t.mode = ThemeMode::System;
        assert_eq!(
            resolve(&t, &sys, Some(ThemeMode::Dark)).accent,
            rgb(0xff8800)
        );
        // Выключено — свой акцент.
        t.system_accent = false;
        assert_eq!(resolve(&t, &sys, None).accent, rgb(0xff8800));
        // Система акцент не сообщает — свой акцент, а без него — палитры.
        t.system_accent = true;
        let no_accent = System {
            accent: None,
            ..sys
        };
        assert_eq!(resolve(&t, &no_accent, None).accent, rgb(0xff8800));
        t.dark_accent.clear();
        assert_eq!(resolve(&t, &no_accent, None), find("breeze").dark);
    }

    #[test]
    fn system_mode() {
        let t = ThemeSettings::default();
        // Портала нет — тёмная.
        assert!(resolve(&t, &System::default(), None).dark);
        let light = System {
            available: true,
            dark: Some(false),
            accent: None,
        };
        assert!(!resolve(&t, &light, None).dark);
        // Портал есть, но предпочтения нет — тёмная.
        let no_preference = System {
            dark: None,
            ..light
        };
        assert!(resolve(&t, &no_preference, None).dark);
        // Портал недоступен — то, что в нём лежит, не учитывается.
        let stale = System {
            available: false,
            ..light
        };
        assert!(resolve(&t, &stale, None).dark);
    }

    #[test]
    fn system_from_portal() {
        use shiftfix::portal::Appearance;
        let t = ThemeSettings {
            system_accent: true,
            ..ThemeSettings::default()
        };
        let ready =
            |scheme, accent| System::from(&PortalState::Ready(Appearance { scheme, accent }));
        // Ещё не ответил или недоступен — как без портала: тёмная.
        for state in [PortalState::Pending, PortalState::Unavailable("нет".into())] {
            let sys = System::from(&state);
            assert_eq!(sys, System::default(), "{state:?}");
            assert_eq!(resolve(&t, &sys, None), find("breeze").dark, "{state:?}");
        }
        assert!(!resolve(&t, &ready(Some(Scheme::Light), None), None).dark);
        assert!(resolve(&t, &ready(Some(Scheme::Dark), None), None).dark);
        // 0 («нет предпочтения») и неизвестный ключ — тёмная (решение автора).
        assert!(resolve(&t, &ready(Some(Scheme::NoPreference), None), None).dark);
        assert!(resolve(&t, &ready(None, None), None).dark);
        // Акцент системы.
        let sys = ready(Some(Scheme::Light), Some([255, 128, 0]));
        assert!(sys.available);
        assert_eq!(sys.accent, Some(rgb(0xff8000)));
        assert_eq!(resolve(&t, &sys, None).accent, rgb(0xff8000));
    }

    #[test]
    fn forced_overrides_mode() {
        let t = ThemeSettings {
            mode: ThemeMode::Light,
            light_palette: "gruvbox".into(),
            dark_palette: "nord".into(),
            ..ThemeSettings::default()
        };
        let none = System::default();
        assert_eq!(resolve(&t, &none, None), find("gruvbox").light);
        assert_eq!(resolve(&t, &none, Some(ThemeMode::Dark)), find("nord").dark);
        let light = System {
            available: true,
            dark: Some(false),
            accent: None,
        };
        let t = ThemeSettings {
            mode: ThemeMode::Dark,
            ..t
        };
        assert_eq!(
            resolve(&t, &light, Some(ThemeMode::System)),
            find("gruvbox").light
        );
        assert_eq!(resolve(&t, &light, None), find("nord").dark);
    }
}
