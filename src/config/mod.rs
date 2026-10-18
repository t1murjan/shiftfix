//! Конфиг: ~/.config/shiftfix/config.toml, все параметры необязательны.
//!
//! `Settings` — значения как в файле (их правит GUI), `Config` — разобранные
//! и проверенные значения для демона.

mod file;
#[cfg(test)]
mod tests;
mod theme;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use evdev::KeyCode;
use log::warn;
use serde::{Deserialize, Serialize};

use crate::devices::Role;
use crate::keymap::norm_layout;
use crate::keys::parse_keys;
use crate::sys::env;
use file::merge_table;
pub use theme::{ThemeMode, ThemeSettings};

/// Конфиг по умолчанию с комментариями: его печатает `shiftfix --print-config`.
pub const DEFAULT_CONFIG: &str = include_str!("default.toml");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub double_tap_ms: u64,
    pub tap_max_hold_ms: u64,
    pub key_delay_ms: u64,
    pub switch_delay_ms: u64,
    pub max_word_len: usize,
    pub selection: bool,
    pub selection_method: String,
    pub max_selection: usize,
    pub paste_keys: String,
    pub paste_restore_ms: u64,
    pub restore_clipboard: bool,
    pub backend: String,
    pub switch_hotkey: String,
    pub hotkey_layouts: Vec<String>,
    pub tray: bool,
    pub notify: bool,
    pub ignore_devices: Vec<String>,
    /// Только для окна: демон секцию не читает. Ошибка в ней не роняет разбор.
    #[serde(default, deserialize_with = "theme::lenient")]
    pub theme: ThemeSettings,
    pub devices: BTreeMap<String, String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            double_tap_ms: 400,
            tap_max_hold_ms: 300,
            key_delay_ms: 5,
            switch_delay_ms: 40,
            max_word_len: 50,
            selection: true,
            selection_method: "paste".into(),
            max_selection: 5000,
            paste_keys: "KEY_LEFTCTRL+KEY_V".into(),
            paste_restore_ms: 400,
            restore_clipboard: true,
            backend: "auto".into(),
            switch_hotkey: "KEY_LEFTMETA+KEY_SPACE".into(),
            hotkey_layouts: vec!["us".into(), "ru".into()],
            tray: true,
            notify: true,
            ignore_devices: vec!["ydotoold virtual device".into()],
            theme: ThemeSettings::default(),
            devices: BTreeMap::new(),
        }
    }
}

/// Ключи верхнего уровня, которые понимает shiftfix, — поля `Settings`.
fn known_keys() -> Vec<String> {
    toml::Table::try_from(Settings::default())
        .map(|table| table.into_iter().map(|(key, _)| key).collect())
        .unwrap_or_default()
}

impl Settings {
    /// Прочитать файл; нет файла — значения по умолчанию.
    pub fn load(path: &Path) -> Result<Settings> {
        let Some((text, table)) = file::read(path)? else {
            return Ok(Settings::default());
        };
        let known = known_keys();
        for key in table.keys().filter(|k| !known.contains(k)) {
            warn!("config: неизвестный параметр {key}");
        }
        toml::from_str(&text).with_context(|| format!("{}: неверное значение", path.display()))
    }

    /// Записать, сохранив комментарии и порядок строк файла (см. `file::edit`:
    /// битый файл не перезаписывается).
    pub fn save(&self, path: &Path) -> Result<()> {
        let fresh: toml_edit::DocumentMut = toml::to_string(self)?
            .parse()
            .context("сериализация конфига")?;
        file::edit(path, |doc| {
            for (key, item) in fresh.iter() {
                if key == "devices" {
                    continue;
                }
                if let Some(table) = item.as_table() {
                    // [theme]: слить, чтобы не терять комментарии внутри таблицы.
                    merge_table(doc, key, table);
                    continue;
                }
                match doc.get_mut(key) {
                    Some(slot) if slot.is_value() => file::set_value(slot, item),
                    _ => {
                        doc.insert(key, item.clone());
                    }
                }
            }
            // [devices] заменяется целиком: удалённая роль устройства должна исчезнуть.
            let mut devices = toml_edit::Table::new();
            for (key, role) in &self.devices {
                devices.insert(key, toml_edit::value(role.as_str()));
            }
            if let Some(old) = doc.get("devices").and_then(toml_edit::Item::as_table) {
                *devices.decor_mut() = old.decor().clone();
            }
            doc.insert("devices", toml_edit::Item::Table(devices));
        })
    }
}

/// Как заменять выделенный текст (`selection_method`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionMethod {
    /// Вставить через буфер обмена.
    Paste,
    /// Перенабрать клавишами.
    Type,
}

impl SelectionMethod {
    pub const ALL: [SelectionMethod; 2] = [SelectionMethod::Paste, SelectionMethod::Type];

    /// Значение в config.toml.
    pub fn as_str(self) -> &'static str {
        match self {
            SelectionMethod::Paste => "paste",
            SelectionMethod::Type => "type",
        }
    }

    pub fn parse(s: &str) -> Option<SelectionMethod> {
        SelectionMethod::ALL.into_iter().find(|m| m.as_str() == s)
    }

    /// Название в окне.
    pub fn title(self) -> &'static str {
        match self {
            SelectionMethod::Paste => "Вставкой",
            SelectionMethod::Type => "Набором",
        }
    }
}

/// Чем переключать раскладку (`backend`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    Auto,
    Kde,
    Gnome,
    Hyprland,
    Sway,
    X11,
    Hotkey,
}

impl BackendKind {
    pub const ALL: [BackendKind; 7] = [
        BackendKind::Auto,
        BackendKind::Kde,
        BackendKind::Gnome,
        BackendKind::Hyprland,
        BackendKind::Sway,
        BackendKind::X11,
        BackendKind::Hotkey,
    ];

    /// Значение в config.toml.
    pub fn as_str(self) -> &'static str {
        match self {
            BackendKind::Auto => "auto",
            BackendKind::Kde => "kde",
            BackendKind::Gnome => "gnome",
            BackendKind::Hyprland => "hyprland",
            BackendKind::Sway => "sway",
            BackendKind::X11 => "x11",
            BackendKind::Hotkey => "hotkey",
        }
    }

    pub fn parse(s: &str) -> Option<BackendKind> {
        BackendKind::ALL.into_iter().find(|b| b.as_str() == s)
    }

    /// Название в окне.
    pub fn title(self) -> &'static str {
        match self {
            BackendKind::Auto => "Автоматически",
            BackendKind::Kde => "KDE Plasma (KWin)",
            BackendKind::Gnome => "GNOME",
            BackendKind::Hyprland => "Hyprland",
            BackendKind::Sway => "Sway",
            BackendKind::X11 => "X11 (XKB)",
            BackendKind::Hotkey => "Нажатием сочетания клавиш",
        }
    }
}

/// «a, b или c» — допустимые значения для сообщения об ошибке.
fn one_of(names: impl IntoIterator<Item = &'static str>) -> String {
    let names: Vec<&str> = names.into_iter().collect();
    match names.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} или {last}", rest.join(", ")),
        _ => names.concat(),
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    /// Секунды: события evdev приходят с меткой времени в секундах.
    pub double_tap: f64,
    pub tap_max_hold: f64,
    pub key_delay: Duration,
    pub switch_delay: Duration,
    pub max_word_len: usize,
    pub selection: bool,
    pub selection_method: SelectionMethod,
    pub max_selection: usize,
    pub paste_keys: Vec<KeyCode>,
    pub paste_restore: Duration,
    pub restore_clipboard: bool,
    pub backend: BackendKind,
    pub switch_hotkey: Vec<KeyCode>,
    pub hotkey_layouts: Vec<String>,
    pub tray: bool,
    pub notify: bool,
    pub ignore_devices: Vec<String>,
    pub devices: BTreeMap<String, Role>,
}

impl Default for Config {
    fn default() -> Self {
        Config::from_settings(Settings::default()).expect("встроенный конфиг корректен")
    }
}

impl Config {
    /// `~/.config/shiftfix/config.toml`.
    pub fn default_path() -> PathBuf {
        env::config_home().join("shiftfix").join("config.toml")
    }

    pub fn load(path: &Path) -> Result<Config> {
        let settings = Settings::load(path)?;
        Config::from_settings(settings).with_context(|| path.display().to_string())
    }

    pub fn from_settings(raw: Settings) -> Result<Config> {
        let Some(selection_method) = SelectionMethod::parse(&raw.selection_method) else {
            bail!(
                "selection_method = {:?}: допустимо {}",
                raw.selection_method,
                one_of(SelectionMethod::ALL.map(SelectionMethod::as_str))
            );
        };
        let Some(backend) = BackendKind::parse(&raw.backend) else {
            bail!(
                "backend = {:?}: допустимо {}",
                raw.backend,
                one_of(BackendKind::ALL.map(BackendKind::as_str))
            );
        };
        let mut devices = BTreeMap::new();
        for (device, role) in &raw.devices {
            let Some(parsed) = Role::parse(role) else {
                bail!(
                    "devices.{device:?} = {role:?}: допустимо {}",
                    one_of(Role::CHOICES.map(Role::as_str))
                );
            };
            devices.insert(device.clone(), parsed);
        }
        let ms = |v: u64| Duration::from_millis(v);
        Ok(Config {
            double_tap: raw.double_tap_ms as f64 / 1000.0,
            tap_max_hold: raw.tap_max_hold_ms as f64 / 1000.0,
            key_delay: ms(raw.key_delay_ms),
            switch_delay: ms(raw.switch_delay_ms),
            max_word_len: raw.max_word_len,
            selection: raw.selection,
            selection_method,
            max_selection: raw.max_selection,
            paste_keys: parse_keys(&raw.paste_keys).context("paste_keys")?,
            paste_restore: ms(raw.paste_restore_ms),
            restore_clipboard: raw.restore_clipboard,
            backend,
            switch_hotkey: parse_keys(&raw.switch_hotkey).context("switch_hotkey")?,
            hotkey_layouts: raw.hotkey_layouts.iter().map(|l| norm_layout(l)).collect(),
            tray: raw.tray,
            notify: raw.notify,
            ignore_devices: raw.ignore_devices,
            devices,
        })
    }
}
