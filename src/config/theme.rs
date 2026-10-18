//! Секция `[theme]`: оформление окна shiftfix-gui. Служба её не читает.

use std::path::Path;

use anyhow::{Context, Result};
use log::warn;
use serde::{Deserialize, Serialize};

use super::file::merge_table;

/// Режим оформления окна.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    /// Как в системе (XDG Desktop Portal).
    #[default]
    System,
    Light,
    Dark,
}

/// Секция `[theme]`: оформление окна shiftfix-gui.
///
/// Акцент — строка, а не `Option`: `save` не удаляет ключи файла, и сброшенный
/// цвет вернулся бы. Пустая строка — акцент палитры.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ThemeSettings {
    pub mode: ThemeMode,
    pub light_palette: String,
    pub dark_palette: String,
    pub light_accent: String,
    pub dark_accent: String,
    /// Только для `mode = "system"`: акцент из системы, если она его сообщает.
    pub system_accent: bool,
}

impl Default for ThemeSettings {
    fn default() -> Self {
        ThemeSettings {
            mode: ThemeMode::System,
            light_palette: "breeze".into(),
            dark_palette: "breeze".into(),
            light_accent: String::new(),
            dark_accent: String::new(),
            system_accent: false,
        }
    }
}

impl ThemeSettings {
    /// Мягкий разбор: неверное поле — предупреждение и значение по умолчанию,
    /// остальные поля читаются как обычно.
    pub(super) fn from_value(value: &toml::Value) -> ThemeSettings {
        let mut theme = ThemeSettings::default();
        let Some(table) = value.as_table() else {
            warn!("config: theme: неверное значение — используется значение по умолчанию");
            return theme;
        };
        for (key, item) in table {
            let ok = match key.as_str() {
                "mode" => read_field(item, &mut theme.mode),
                "light_palette" => read_field(item, &mut theme.light_palette),
                "dark_palette" => read_field(item, &mut theme.dark_palette),
                "light_accent" => read_field(item, &mut theme.light_accent),
                "dark_accent" => read_field(item, &mut theme.dark_accent),
                "system_accent" => read_field(item, &mut theme.system_accent),
                _ => {
                    warn!("config: неизвестный параметр theme.{key}");
                    continue;
                }
            };
            if !ok {
                warn!(
                    "config: theme.{key}: неверное значение — используется значение по умолчанию"
                );
            }
        }
        theme
    }

    /// Прочитать только `[theme]`: ошибка в другом параметре тему не сбрасывает.
    /// Нет файла — значения по умолчанию, ошибка TOML — `Err`.
    pub fn load(path: &Path) -> Result<ThemeSettings> {
        let Some((_, table)) = super::file::read(path)? else {
            return Ok(ThemeSettings::default());
        };
        Ok(table
            .get("theme")
            .map(ThemeSettings::from_value)
            .unwrap_or_default())
    }

    /// Записать только `[theme]`, остальной файл не трогать (см. `file::edit`:
    /// нет файла — основа DEFAULT_CONFIG, битый файл не перезаписывается).
    pub fn save(&self, path: &Path) -> Result<()> {
        let fresh: toml_edit::DocumentMut = toml::to_string(self)?
            .parse()
            .context("сериализация темы")?;
        super::file::edit(path, |doc| merge_table(doc, "theme", fresh.as_table()))
    }

    /// Задан ли в файле `theme.mode` — для переноса режима из старого файла
    /// окна `gui-theme`. Нет файла — `false`; файл не читается или не
    /// разбирается — `Err`.
    pub fn has_mode(path: &Path) -> Result<bool> {
        let table = super::file::read(path)?.map(|(_, table)| table);
        Ok(table.is_some_and(|t| t.get("theme").and_then(|v| v.get("mode")).is_some()))
    }
}

fn read_field<T: serde::de::DeserializeOwned>(item: &toml::Value, slot: &mut T) -> bool {
    match item.clone().try_into() {
        Ok(value) => {
            *slot = value;
            true
        }
        Err(_) => false,
    }
}

/// Кривая секция `[theme]` не роняет ни запуск службы, ни reload.
pub(super) fn lenient<'de, D: serde::Deserializer<'de>>(d: D) -> Result<ThemeSettings, D::Error> {
    Ok(ThemeSettings::from_value(&toml::Value::deserialize(d)?))
}
