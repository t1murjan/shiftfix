//! CLI службы глазами install.sh.

use std::process::Command;

use shiftfix::config::{Settings, ThemeSettings};

/// install.sh создаёт конфиг из `--print-config`: он должен разбираться в
/// значения по умолчанию, включая [theme].
#[test]
fn print_config_parses_to_defaults() {
    let out = Command::new(env!("CARGO_BIN_EXE_shiftfix"))
        .arg("--print-config")
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(
        toml::from_str::<Settings>(&text).unwrap(),
        Settings::default()
    );
    let table: toml::Table = text.parse().unwrap();
    assert_eq!(
        ThemeSettings::from_value(&table["theme"]),
        ThemeSettings::default()
    );
}

/// Без systemd install.sh пишет автозапуск из `--print-autostart`: тот же
/// текст, что и окно (`service::autostart_text`), с путём к этому бинарнику.
#[test]
fn print_autostart_matches_gui_text() {
    let exe = std::fs::canonicalize(env!("CARGO_BIN_EXE_shiftfix")).unwrap();
    let out = Command::new(&exe)
        .arg("--print-autostart")
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(text, shiftfix::service::autostart_text(&exe));
}

/// `shiftfix-gui --version` отвечает до блокировки окна и без графики,
/// и версия та же, что у службы: по ним install.sh проверяет сборку.
#[test]
fn gui_version_without_window() {
    let runtime = std::env::temp_dir().join(format!("shiftfix-cli-{}", std::process::id()));
    std::fs::create_dir_all(&runtime).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_shiftfix-gui"))
        .arg("--version")
        .env("XDG_RUNTIME_DIR", &runtime)
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .output()
        .unwrap();
    let lock_created = runtime.join("shiftfix-gui.lock").exists();
    std::fs::remove_dir_all(&runtime).unwrap();
    assert!(out.status.success(), "{out:?}");
    let version = env!("CARGO_PKG_VERSION");
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        format!("shiftfix-gui {version}\n")
    );
    assert!(!lock_created, "--version не должен брать блокировку окна");

    let out = Command::new(env!("CARGO_BIN_EXE_shiftfix"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        format!("shiftfix {version}\n")
    );
}
