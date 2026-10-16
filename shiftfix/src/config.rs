//! Конфиг: ~/.config/shiftfix/config.toml, все параметры необязательны.
//!
//! `Settings` — значения как в файле (их правит GUI), `Config` — разобранные
//! и проверенные значения для демона.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use evdev::KeyCode;
use log::warn;
use serde::{Deserialize, Serialize};

use crate::keys::{norm_layout, parse_keys};

pub const DEFAULT_CONFIG: &str = r#"# ~/.config/shiftfix/config.toml — все параметры необязательны.
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
            devices: BTreeMap::new(),
        }
    }
}

const KNOWN_KEYS: [&str; 18] = [
    "double_tap_ms",
    "tap_max_hold_ms",
    "key_delay_ms",
    "switch_delay_ms",
    "max_word_len",
    "selection",
    "selection_method",
    "max_selection",
    "paste_keys",
    "paste_restore_ms",
    "restore_clipboard",
    "backend",
    "switch_hotkey",
    "hotkey_layouts",
    "tray",
    "notify",
    "ignore_devices",
    "devices",
];

impl Settings {
    /// Прочитать файл; нет файла — значения по умолчанию.
    pub fn load(path: &Path) -> Result<Settings> {
        if !path.exists() {
            return Ok(Settings::default());
        }
        let text =
            fs::read_to_string(path).with_context(|| format!("не читается {}", path.display()))?;
        let table: toml::Table = text
            .parse()
            .with_context(|| format!("{}: ошибка TOML", path.display()))?;
        for key in table.keys().filter(|k| !KNOWN_KEYS.contains(&k.as_str())) {
            warn!("config: неизвестный параметр {key}");
        }
        toml::from_str(&text).with_context(|| format!("{}: неверное значение", path.display()))
    }

    /// Записать, сохранив комментарии и порядок строк файла.
    pub fn save(&self, path: &Path) -> Result<()> {
        let original = fs::read_to_string(path).unwrap_or_else(|_| DEFAULT_CONFIG.to_owned());
        let mut doc: toml_edit::DocumentMut = original
            .parse()
            .or_else(|_| DEFAULT_CONFIG.parse())
            .context("не удалось разобрать конфиг для записи")?;
        let fresh: toml_edit::DocumentMut = toml::to_string(self)?
            .parse()
            .context("сериализация конфига")?;
        for (key, item) in fresh.iter() {
            if key == "devices" {
                continue;
            }
            match doc.get_mut(key) {
                Some(slot) if slot.is_value() => {
                    // Значение меняем, а комментарий над строкой оставляем.
                    let decor = slot.as_value().map(|v| v.decor().clone());
                    *slot = item.clone();
                    if let (Some(decor), Some(value)) = (decor, slot.as_value_mut()) {
                        *value.decor_mut() = decor;
                    }
                }
                _ => {
                    doc.insert(key, item.clone());
                }
            }
        }
        let mut devices = toml_edit::Table::new();
        for (key, role) in &self.devices {
            devices.insert(key, toml_edit::value(role.as_str()));
        }
        if let Some(old) = doc.get("devices").and_then(toml_edit::Item::as_table) {
            *devices.decor_mut() = old.decor().clone();
        }
        doc.insert("devices", toml_edit::Item::Table(devices));

        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).with_context(|| format!("не создаётся {}", dir.display()))?;
        }
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, doc.to_string())
            .with_context(|| format!("не записывается {}", tmp.display()))?;
        fs::rename(&tmp, path).with_context(|| format!("не записывается {}", path.display()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionMethod {
    Paste,
    Type,
}

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
    pub devices: BTreeMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        Config::from_settings(Settings::default()).expect("встроенный конфиг корректен")
    }
}

impl Config {
    pub fn default_path() -> PathBuf {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("shiftfix").join("config.toml")
    }

    pub fn load(path: &Path) -> Result<Config> {
        let settings = Settings::load(path)?;
        Config::from_settings(settings).with_context(|| path.display().to_string())
    }

    pub fn from_settings(raw: Settings) -> Result<Config> {
        let selection_method = match raw.selection_method.as_str() {
            "paste" => SelectionMethod::Paste,
            "type" => SelectionMethod::Type,
            other => bail!("selection_method = {other:?}: допустимо paste или type"),
        };
        let backend = match raw.backend.as_str() {
            "auto" => BackendKind::Auto,
            "kde" => BackendKind::Kde,
            "gnome" => BackendKind::Gnome,
            "hyprland" => BackendKind::Hyprland,
            "sway" => BackendKind::Sway,
            "x11" => BackendKind::X11,
            "hotkey" => BackendKind::Hotkey,
            other => bail!(
                "backend = {other:?}: допустимо auto, kde, gnome, hyprland, sway, x11 или hotkey"
            ),
        };
        for (device, role) in &raw.devices {
            if crate::devices::Role::parse(role).is_none() {
                bail!(
                    "devices.{device:?} = {role:?}: допустимо keyboard, mouse, touchpad, touchscreen, tablet или ignore"
                );
            }
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
            devices: raw.devices,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("shiftfix-config-{name}-{}", std::process::id()));
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
        let mut keys: Vec<&str> = table.keys().map(String::as_str).collect();
        let mut known = KNOWN_KEYS.to_vec();
        keys.sort_unstable();
        known.sort_unstable();
        assert_eq!(keys, known);
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
}
