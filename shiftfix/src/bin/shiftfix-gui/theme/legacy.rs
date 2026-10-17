//! Перенос режима окна из старого файла `gui-theme` (до версии 0.4) в `[theme]`.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use shiftfix::config::{ThemeMode, ThemeSettings};

use super::parse_mode;

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
