//! CLI службы глазами install.sh.

use std::os::unix::fs::MetadataExt;
use std::process::{Command, Stdio};

/// install.sh создаёт конфиг из `--print-config`: это должен быть ровно
/// config/default.toml — его тесты config сверяют с `Settings::default()`.
#[test]
fn print_config_is_default_config() {
    let out = Command::new(env!("CARGO_BIN_EXE_shiftfix"))
        .arg("--print-config")
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        shiftfix::config::DEFAULT_CONFIG
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

/// Под systemd stderr службы — журнал: приоритет у каждой строки, и у
/// многострочной ошибки разбора TOML тоже. По нему окно находит причину
/// остановки службы (`parse_journal` в shiftfix-gui).
#[test]
fn journal_gets_priority_on_every_line() {
    let dir = std::env::temp_dir().join(format!("shiftfix-cli-journal-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let config = dir.join("config.toml");
    std::fs::write(&config, "key_delay_ms = \"x\"\n").unwrap();
    // Журнал служба узнаёт по JOURNAL_STREAM: устройство и inode её stderr.
    let log = dir.join("journal");
    let stderr = std::fs::File::create(&log).unwrap();
    let meta = stderr.metadata().unwrap();
    let status = Command::new(env!("CARGO_BIN_EXE_shiftfix"))
        .arg("--config")
        .arg(&config)
        .env("XDG_RUNTIME_DIR", &dir)
        .env("JOURNAL_STREAM", format!("{}:{}", meta.dev(), meta.ino()))
        .stderr(stderr)
        .status()
        .unwrap();
    let journal = std::fs::read_to_string(&log).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(status.code(), Some(shiftfix::service::EXIT_SETUP.into()));
    assert!(journal.starts_with("<3>config: "), "{journal}");
    assert!(journal.lines().count() > 1, "{journal}");
    assert!(journal.lines().all(|l| l.starts_with("<3>")), "{journal}");
}

/// Служба, запущенная из терминала, после его закрытия не может писать в
/// stderr. Журнал это переживает: ошибка конфига даёт свой код выхода, а не
/// панику (101).
#[test]
fn closed_stderr_is_not_fatal() {
    let dir = std::env::temp_dir().join(format!("shiftfix-cli-stderr-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let config = dir.join("config.toml");
    std::fs::write(&config, "backend = \"windows\"\n").unwrap();
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader); // запись в stderr — EPIPE
    let status = Command::new(env!("CARGO_BIN_EXE_shiftfix"))
        .arg("--config")
        .arg(&config)
        .env("XDG_RUNTIME_DIR", &dir)
        .env_remove("JOURNAL_STREAM")
        .stdout(Stdio::null())
        .stderr(writer)
        .status()
        .unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(status.code(), Some(shiftfix::service::EXIT_SETUP.into()));
}

/// `--check` с ошибкой в конфиге сообщает её первой и проверяет остальное с
/// настройками по умолчанию; код выхода — прежний, как у службы.
#[test]
fn check_reports_config_error_and_checks_the_rest() {
    let dir = std::env::temp_dir().join(format!("shiftfix-cli-check-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let config = dir.join("config.toml");
    std::fs::write(&config, "key_delay_ms = \"x\"\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_shiftfix"))
        .arg("--check")
        .arg("--config")
        .arg(&config)
        .env("XDG_RUNTIME_DIR", &dir)
        .output()
        .unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.starts_with("Конфиг\n  ✗ "), "{text}");
    assert!(text.contains("key_delay_ms"), "{text}");
    assert!(text.contains("\nУстройства ввода\n"), "{text}");
    assert!(text.contains("\nПроблем: "), "{text}");
    assert_eq!(
        out.status.code(),
        Some(shiftfix::service::EXIT_SETUP.into())
    );
}
