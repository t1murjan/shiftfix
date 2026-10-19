use std::time::Instant;

use shiftfix::config::{ThemeMode, ThemeSettings};

use super::*;
use crate::portal::{PortalState, Scheme};
use palettes::{WHITE, rgb};

fn own_accent(mode: ThemeMode, light: &str, dark: &str) -> ThemeSettings {
    ThemeSettings {
        mode,
        light_accent: light.into(),
        dark_accent: dark.into(),
        ..ThemeSettings::default()
    }
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
    use crate::portal::Appearance;
    let t = ThemeSettings {
        system_accent: true,
        ..ThemeSettings::default()
    };
    let ready = |scheme, accent| System::from(&PortalState::Ready(Appearance { scheme, accent }));
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

#[test]
fn unknown_palette_names() {
    assert!(unknown_palettes(&ThemeSettings::default()).is_empty());
    let t = ThemeSettings {
        light_palette: "solarized".into(),
        dark_palette: " Nord ".into(),
        ..ThemeSettings::default()
    };
    // Имя без учёта регистра и пробелов по краям — как в `find`.
    assert_eq!(unknown_palettes(&t), ["solarized"]);
    assert_eq!(
        resolve(&t, &System::default(), Some(ThemeMode::Light)),
        find("breeze").light
    );
}
