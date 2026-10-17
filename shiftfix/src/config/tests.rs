use super::*;

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("shiftfix-config-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    dir.join("config.toml")
}

#[test]
fn default_text_matches_defaults() {
    assert_eq!(
        toml::from_str::<Settings>(DEFAULT_CONFIG).unwrap(),
        Settings::default()
    );
    let table: toml::Table = DEFAULT_CONFIG.parse().unwrap();
    let mut keys: Vec<String> = table.keys().cloned().collect();
    let mut known = known_keys();
    assert_eq!(known.len(), 19);
    keys.sort_unstable();
    known.sort_unstable();
    assert_eq!(keys, known);
    // [theme] в тексте перечисляет все поля темы и стоит перед [devices].
    let theme_keys: Vec<&String> = table["theme"].as_table().unwrap().keys().collect();
    let defaults = toml::Value::try_from(ThemeSettings::default()).unwrap();
    let field_keys: Vec<&String> = defaults.as_table().unwrap().keys().collect();
    assert_eq!(theme_keys, field_keys);
    let pos = |s: &str| DEFAULT_CONFIG.find(s).unwrap();
    assert!(pos("\n[theme]\n") < pos("# Роли устройств"));
    assert!(DEFAULT_CONFIG.ends_with("[devices]\n"));

    // `--print-config > config.toml`, как делает install.sh.
    let path = temp("print-config");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, DEFAULT_CONFIG).unwrap();
    assert_eq!(Settings::load(&path).unwrap(), Settings::default());
    assert_eq!(
        ThemeSettings::load(&path).unwrap(),
        ThemeSettings::default()
    );
    Config::load(&path).unwrap();
    fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[test]
fn partial_config_and_errors() {
    let raw: Settings = toml::from_str("double_tap_ms = 250\nbackend = \"kde\"").unwrap();
    let cfg = Config::from_settings(raw).unwrap();
    assert_eq!(cfg.double_tap, 0.25);
    assert_eq!(cfg.backend, BackendKind::Kde);
    assert_eq!(cfg.max_word_len, 50);
    let bad: Settings = toml::from_str("selection_method = \"clipboard\"").unwrap();
    assert!(Config::from_settings(bad).is_err());
    let bad: Settings = toml::from_str("paste_keys = \"ctrl+v\"").unwrap();
    assert!(Config::from_settings(bad).is_err());
    let bad: Settings = toml::from_str("[devices]\n\"1:2 x\" = \"screen\"").unwrap();
    assert!(Config::from_settings(bad).is_err());
}

#[test]
fn save_keeps_comments_and_round_trips() {
    let path = temp("save");
    let mut settings = Settings {
        double_tap_ms: 333,
        tray: false,
        ..Settings::default()
    };
    settings
        .devices
        .insert("046d:c52b Logitech USB Receiver".into(), "ignore".into());
    settings.save(&path).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("# Двойной Shift: второе нажатие"), "{text}");
    assert!(text.contains("double_tap_ms = 333"), "{text}");
    assert!(
        text.contains("\"046d:c52b Logitech USB Receiver\" = \"ignore\""),
        "{text}"
    );
    assert_eq!(Settings::load(&path).unwrap(), settings);

    // Повторное сохранение не плодит дубли и не трогает чужие комментарии.
    fs::write(
        &path,
        text.replace("tray = false", "# мой комментарий\ntray = false"),
    )
    .unwrap();
    settings.devices.clear();
    settings.save(&path).unwrap();
    let again = fs::read_to_string(&path).unwrap();
    assert!(again.contains("# мой комментарий"), "{again}");
    assert_eq!(
        again
            .lines()
            .filter(|l| l.starts_with("double_tap_ms"))
            .count(),
        1
    );
    assert!(!again.contains("Logitech"));
    assert_eq!(Settings::load(&path).unwrap(), settings);
    fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

/// config.toml версии 0.3.0: тогдашний `--print-config`, без [theme].
const CONFIG_0_3_0: &str = r#"# ~/.config/shiftfix/config.toml — все параметры необязательны.
# Удобнее менять их в программе shiftfix-gui, вкладка «Настройки».

# Двойной Shift: второе нажатие должно начаться не позже double_tap_ms после
# отпускания первого, и каждое нажатие короче tap_max_hold_ms.
double_tap_ms = 400
tap_max_hold_ms = 300

# Пауза между эмулируемыми событиями клавиатуры и после смены раскладки.
# Если в каком-то приложении теряются буквы — увеличь key_delay_ms.
key_delay_ms = 5
switch_delay_ms = 40

# Слова длиннее не трогаем (страховка от игр, где «слово» — это WASD).
max_word_len = 50

# Исправление выделенного текста (PRIMARY): в Wayland нужен wl-clipboard,
# в X11 и GNOME — xclip.
selection = true
# paste — вставка через буфер обмена; type — перенабор клавишами: без буфера
# обмена, но в нередактируемом месте нажатия уйдут как горячие клавиши сайта,
# а переводы строк и символы, которых нет на клавиатуре, всё равно вставятся
# через буфер обмена.
selection_method = "paste"
max_selection = 5000
paste_keys = "KEY_LEFTCTRL+KEY_V"
paste_restore_ms = 400
restore_clipboard = true

# auto | kde | gnome | hyprland | sway | x11 | hotkey
backend = "auto"
# Только для backend = "hotkey": сочетание переключения раскладки и раскладки по порядку.
switch_hotkey = "KEY_LEFTMETA+KEY_SPACE"
hotkey_layouts = ["us", "ru"]

# Значок в системном трее и уведомления о паузе.
tray = true
notify = true

# Устройства, чьи нажатия не учитываются (имена — из `shiftfix --check`).
ignore_devices = ["ydotoold virtual device"]

# Роли устройств, заданные вручную (вкладка «Устройства» в shiftfix-gui).
# Ключ — «vendor:product имя», значение — keyboard, mouse, touchpad,
# touchscreen, tablet или ignore.
[devices]
"#;

/// Старый конфиг со своими значениями и ролями устройств.
const CUSTOM_0_3_0: &str = r#"# мой конфиг
double_tap_ms = 350
key_delay_ms = 7
backend = "kde"
hotkey_layouts = ["us", "ru", "de"]

[devices]
"046d:c52b Logitech USB Receiver" = "ignore"
"#;

fn write(name: &str, text: &str) -> PathBuf {
    let path = temp(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, text).unwrap();
    path
}

fn cleanup(path: &Path) {
    fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

fn count_lines(text: &str, pred: impl Fn(&str) -> bool) -> usize {
    text.lines().filter(|l| pred(l)).count()
}

#[test]
fn old_config_without_theme() {
    for (name, text) in [("old", CONFIG_0_3_0), ("old-custom", CUSTOM_0_3_0)] {
        let path = write(name, text);
        let settings = Settings::load(&path).unwrap();
        assert_eq!(settings.theme, ThemeSettings::default());
        assert_eq!(
            ThemeSettings::load(&path).unwrap(),
            ThemeSettings::default()
        );
        Config::from_settings(settings).unwrap();
        Config::load(&path).unwrap();
        cleanup(&path);
    }
    assert_eq!(
        toml::from_str::<Settings>(CONFIG_0_3_0).unwrap(),
        Settings::default()
    );
    let custom: Settings = toml::from_str(CUSTOM_0_3_0).unwrap();
    assert_eq!((custom.double_tap_ms, custom.key_delay_ms), (350, 7));
    assert_eq!(custom.devices.len(), 1);
}

#[test]
fn partial_theme_uses_defaults() {
    let s: Settings = toml::from_str("[theme]\nmode = \"dark\"\n").unwrap();
    assert_eq!(
        s.theme,
        ThemeSettings {
            mode: ThemeMode::Dark,
            ..ThemeSettings::default()
        }
    );
    assert_eq!(
        s,
        Settings {
            theme: s.theme.clone(),
            ..Settings::default()
        }
    );
}

#[test]
fn bad_theme_values_fall_back() {
    let text = "key_delay_ms = 7\n[theme]\nmode = 1\nlight_accent = 5\n\
                dark_palette = \"nord\"\nsystem_accent = \"да\"\nmdoe = \"dark\"\n";
    let path = write("bad-theme", text);
    let settings = Settings::load(&path).unwrap();
    assert_eq!(settings.key_delay_ms, 7);
    assert_eq!(
        settings.theme,
        ThemeSettings {
            dark_palette: "nord".into(),
            ..ThemeSettings::default()
        }
    );
    assert_eq!(ThemeSettings::load(&path).unwrap(), settings.theme);
    Config::from_settings(settings).unwrap();
    Config::load(&path).unwrap();
    cleanup(&path);

    for text in [
        "theme = 5\n",
        "theme = \"dark\"\n",
        "[theme]\nmode = \"Dark\"\n",
    ] {
        let s: Settings = toml::from_str(text).unwrap();
        assert_eq!(s.theme, ThemeSettings::default(), "{text}");
    }

    // Ошибка в чужом параметре тему не сбрасывает.
    let path = write(
        "foreign-error",
        "key_delay_ms = \"x\"\n[theme]\nmode = \"light\"\n",
    );
    assert!(Settings::load(&path).is_err());
    assert_eq!(ThemeSettings::load(&path).unwrap().mode, ThemeMode::Light);
    cleanup(&path);
}

#[test]
fn theme_round_trips() {
    let settings = Settings {
        theme: ThemeSettings {
            mode: ThemeMode::Light,
            light_palette: "nord".into(),
            dark_palette: "gruvbox".into(),
            light_accent: "#ff8800".into(),
            dark_accent: "#3daee9".into(),
            system_accent: true,
        },
        ..Settings::default()
    };
    let text = toml::to_string(&settings).unwrap();
    assert_eq!(toml::from_str::<Settings>(&text).unwrap(), settings);
    for mode in [ThemeMode::System, ThemeMode::Light, ThemeMode::Dark] {
        let theme = ThemeSettings {
            mode,
            ..settings.theme.clone()
        };
        let value = toml::Value::try_from(&theme).unwrap();
        assert_eq!(ThemeSettings::from_value(&value), theme);
    }
}

#[test]
fn theme_save_touches_only_theme() {
    let old = CONFIG_0_3_0.replace("double_tap_ms = 400", "double_tap_ms = 350");
    let path = write("theme-save", &old);
    let theme = ThemeSettings {
        mode: ThemeMode::Dark,
        dark_palette: "nord".into(),
        ..ThemeSettings::default()
    };
    theme.save(&path).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.starts_with(&old), "{text}");
    assert_eq!(count_lines(&text, |l| l == "[theme]"), 1, "{text}");
    assert!(text.contains("# Двойной Shift"), "{text}");
    assert!(text.contains("# Роли устройств"), "{text}");
    assert!(text.contains("double_tap_ms = 350"), "{text}");
    let settings = Settings::load(&path).unwrap();
    assert_eq!(settings.theme, theme);
    assert_eq!(settings.double_tap_ms, 350);
    assert_eq!(ThemeSettings::load(&path).unwrap(), theme);

    // Повторное сохранение: тот же текст байт в байт, файл не переписывается.
    use std::os::unix::fs::MetadataExt;
    let inode = fs::metadata(&path).unwrap().ino();
    theme.save(&path).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), text);
    assert_eq!(fs::metadata(&path).unwrap().ino(), inode);

    // Новая тема снова меняет только [theme].
    let light = ThemeSettings {
        mode: ThemeMode::Light,
        ..theme.clone()
    };
    light.save(&path).unwrap();
    let again = fs::read_to_string(&path).unwrap();
    assert!(again.starts_with(&old), "{again}");
    assert_eq!(count_lines(&again, |l| l == "[theme]"), 1, "{again}");
    assert_eq!(count_lines(&again, |l| l.starts_with("mode =")), 1);
    assert_eq!(ThemeSettings::load(&path).unwrap(), light);
    cleanup(&path);

    // Нет файла — основа DEFAULT_CONFIG со всеми комментариями.
    let path = temp("theme-new");
    theme.save(&path).unwrap();
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        DEFAULT_CONFIG
            .replace("\nmode = \"system\"\n", "\nmode = \"dark\"\n")
            .replace("dark_palette = \"breeze\"", "dark_palette = \"nord\"")
    );
    assert_eq!(
        Settings::load(&path).unwrap(),
        Settings {
            theme: theme.clone(),
            ..Settings::default()
        }
    );
    cleanup(&path);
}

#[test]
fn accent_reset_reaches_file() {
    let path = write("accent", CONFIG_0_3_0);
    let mut theme = ThemeSettings {
        light_accent: "#ff8800".into(),
        ..ThemeSettings::default()
    };
    theme.save(&path).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("light_accent = \"#ff8800\""), "{text}");
    theme.light_accent.clear();
    theme.save(&path).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert!(!text.contains("#ff8800"), "{text}");
    assert!(text.contains("light_accent = \"\""), "{text}");
    assert_eq!(
        ThemeSettings::load(&path).unwrap(),
        ThemeSettings::default()
    );

    // То же через общий Settings::save.
    let mut settings = Settings::load(&path).unwrap();
    settings.theme.dark_accent = "#123456".into();
    settings.save(&path).unwrap();
    assert!(fs::read_to_string(&path).unwrap().contains("#123456"));
    settings.theme.dark_accent.clear();
    settings.save(&path).unwrap();
    assert!(!fs::read_to_string(&path).unwrap().contains("#123456"));
    assert_eq!(Settings::load(&path).unwrap(), settings);
    cleanup(&path);
}

#[test]
fn theme_save_refuses_broken_toml() {
    let broken = "double_tap_ms = = 1\n[theme\nmode = \"dark\"\n";
    let path = write("broken", broken);
    let err = ThemeSettings::default().save(&path).unwrap_err();
    assert!(format!("{err:#}").contains("ошибка TOML"), "{err:#}");
    assert_eq!(fs::read(&path).unwrap(), broken.as_bytes());
    assert!(!path.with_extension("toml.tmp").exists());
    assert!(ThemeSettings::load(&path).is_err());
    cleanup(&path);
}

#[test]
fn settings_save_merges_theme() {
    let text = DEFAULT_CONFIG
        .replace(
            "\nmode = \"system\"\n",
            "\n# мой комментарий к теме\nmode = \"dark\" # ночью\n",
        )
        .replace(
            "system_accent = false",
            "system_accent = false\nfuture_key = 1",
        )
        + "\"1:2 mouse\" = \"mouse\"\n\"3:4 pad\" = \"touchpad\"\n";
    let path = write("merge", &text);
    let mut settings = Settings::load(&path).unwrap();
    assert_eq!(settings.theme.mode, ThemeMode::Dark);
    assert_eq!(settings.devices.len(), 2);
    settings.theme.light_palette = "gruvbox".into();
    settings.double_tap_ms = 250;
    settings.devices.remove("1:2 mouse");
    settings.save(&path).unwrap();
    let saved = fs::read_to_string(&path).unwrap();
    settings.save(&path).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), saved);

    assert!(
        saved.contains("# мой комментарий к теме\nmode = \"dark\" # ночью\n"),
        "{saved}"
    );
    assert!(saved.contains("# Палитра светлого и тёмного"), "{saved}");
    assert!(saved.contains("light_palette = \"gruvbox\""), "{saved}");
    // Ключ от будущей версии в [theme] остаётся, удалённое устройство исчезает.
    assert!(saved.contains("future_key = 1"), "{saved}");
    assert!(!saved.contains("1:2 mouse"), "{saved}");
    assert!(saved.contains("\"3:4 pad\" = \"touchpad\""), "{saved}");
    for header in ["[theme]", "[devices]"] {
        assert_eq!(count_lines(&saved, |l| l == header), 1, "{saved}");
    }
    for key in ["mode", "light_palette", "future_key", "double_tap_ms"] {
        let prefix = format!("{key} =");
        assert_eq!(
            count_lines(&saved, |l| l.starts_with(&prefix)),
            1,
            "{saved}"
        );
    }
    assert!(
        saved.find("[theme]").unwrap() < saved.find("[devices]").unwrap(),
        "{saved}"
    );
    assert_eq!(Settings::load(&path).unwrap(), settings);
    cleanup(&path);

    // Старый конфиг без [theme]: таблица появляется один раз, роли и
    // комментарии на месте, повторное сохранение ничего не меняет.
    let path = write("merge-old", CUSTOM_0_3_0);
    let mut settings = Settings::load(&path).unwrap();
    settings.theme.mode = ThemeMode::Light;
    settings.save(&path).unwrap();
    let saved = fs::read_to_string(&path).unwrap();
    settings.save(&path).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), saved);
    assert!(saved.starts_with("# мой конфиг\n"), "{saved}");
    assert_eq!(count_lines(&saved, |l| l == "[theme]"), 1, "{saved}");
    assert!(saved.contains("\"046d:c52b Logitech USB Receiver\" = \"ignore\""));
    assert_eq!(Settings::load(&path).unwrap(), settings);
    cleanup(&path);
}

#[test]
fn save_through_symlink_keeps_link_and_mode() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let path = temp("symlink");
    let dir = path.parent().unwrap().to_path_buf();
    let real = dir.join("dotfiles").join("shiftfix.toml");
    fs::create_dir_all(real.parent().unwrap()).unwrap();
    fs::write(&real, CONFIG_0_3_0).unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o600)).unwrap();
    symlink("dotfiles/shiftfix.toml", &path).unwrap();

    let theme = ThemeSettings {
        mode: ThemeMode::Light,
        ..ThemeSettings::default()
    };
    theme.save(&path).unwrap();
    let mut settings = Settings::load(&path).unwrap();
    assert_eq!(settings.theme, theme);
    settings.key_delay_ms = 9;
    settings.save(&path).unwrap();

    assert!(fs::symlink_metadata(&path).unwrap().is_symlink());
    assert_eq!(
        fs::metadata(&real).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(Settings::load(&real).unwrap(), settings);

    // Висячая ссылка: файл создаётся там, куда она указывает.
    let dangling = dir.join("dangling.toml");
    symlink("dotfiles/new/config.toml", &dangling).unwrap();
    theme.save(&dangling).unwrap();
    assert!(fs::symlink_metadata(&dangling).unwrap().is_symlink());
    assert_eq!(ThemeSettings::load(&dangling).unwrap(), theme);
    assert!(dir.join("dotfiles/new/config.toml").is_file());

    let mut names = Vec::new();
    for sub in ["", "dotfiles", "dotfiles/new"] {
        for entry in fs::read_dir(dir.join(sub)).unwrap() {
            names.push(entry.unwrap().file_name().into_string().unwrap());
        }
    }
    assert!(!names.iter().any(|n| n.ends_with(".tmp")), "{names:?}");
    cleanup(&path);
}
