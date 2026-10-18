//! Устройства ввода: что это (клавиатура, мышь, тачпад, сенсорный экран, перо),
//! кого слушать и как понимать их события.
//!
//! Роль определяется по возможностям устройства (свойства INPUT_PROP_DIRECT /
//! POINTER, кнопки, оси). Если автоматика ошиблась — роль можно задать вручную
//! в GUI, она хранится в `[devices]` конфига.
//!
//! Здесь — что это за устройство; чтение событий — в `registry.rs`.

mod registry;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use evdev::raw_stream::RawDevice;
use evdev::{KeyCode, PropType, RelativeAxisCode};
use serde::{Deserialize, Serialize};

use crate::config::Config;
pub use registry::{Event, Registry, ScanStats};

/// Имя виртуальной клавиатуры службы (`daemon/uinput.rs`): по нему служба не
/// слушает саму себя.
pub const OWN_KEYBOARD: &str = "shiftfix virtual keyboard";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Keyboard,
    Mouse,
    Touchpad,
    Touchscreen,
    Tablet,
    /// Определено, но пользователь попросил не слушать.
    Ignore,
    /// Не клавиатура и не указатель: кнопка питания, датчики, гнёзда звука…
    #[default]
    Other,
}

impl Role {
    pub const CHOICES: [Role; 6] = [
        Role::Keyboard,
        Role::Mouse,
        Role::Touchpad,
        Role::Touchscreen,
        Role::Tablet,
        Role::Ignore,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Role::Keyboard => "keyboard",
            Role::Mouse => "mouse",
            Role::Touchpad => "touchpad",
            Role::Touchscreen => "touchscreen",
            Role::Tablet => "tablet",
            Role::Ignore => "ignore",
            Role::Other => "other",
        }
    }

    pub fn parse(s: &str) -> Option<Role> {
        Role::CHOICES.into_iter().find(|r| r.as_str() == s)
    }

    pub fn title(self) -> &'static str {
        match self {
            Role::Keyboard => "Клавиатура",
            Role::Mouse => "Мышь",
            Role::Touchpad => "Тачпад",
            Role::Touchscreen => "Сенсорный экран",
            Role::Tablet => "Планшет / перо",
            Role::Ignore => "Не слушать",
            Role::Other => "Другое",
        }
    }

    /// Учитываются ли события устройства.
    pub fn listens(self) -> bool {
        !matches!(self, Role::Ignore | Role::Other)
    }

    fn to_u8(self) -> u8 {
        self as u8
    }

    fn from_u8(v: u8) -> Role {
        // CHOICES идут в порядке объявления, Other — последней.
        Role::CHOICES
            .get(usize::from(v))
            .copied()
            .unwrap_or(Role::Other)
    }
}

/// То, что важно для классификации.
#[derive(Clone, Debug, Default)]
pub struct Caps {
    pub keys: HashSet<KeyCode>,
    pub relative: bool,
    pub direct: bool,
    pub pointer: bool,
}

const LETTERS: [KeyCode; 26] = [
    KeyCode::KEY_Q,
    KeyCode::KEY_W,
    KeyCode::KEY_E,
    KeyCode::KEY_R,
    KeyCode::KEY_T,
    KeyCode::KEY_Y,
    KeyCode::KEY_U,
    KeyCode::KEY_I,
    KeyCode::KEY_O,
    KeyCode::KEY_P,
    KeyCode::KEY_A,
    KeyCode::KEY_S,
    KeyCode::KEY_D,
    KeyCode::KEY_F,
    KeyCode::KEY_G,
    KeyCode::KEY_H,
    KeyCode::KEY_J,
    KeyCode::KEY_K,
    KeyCode::KEY_L,
    KeyCode::KEY_Z,
    KeyCode::KEY_X,
    KeyCode::KEY_C,
    KeyCode::KEY_V,
    KeyCode::KEY_B,
    KeyCode::KEY_N,
    KeyCode::KEY_M,
];

/// Роль по возможностям устройства.
pub fn classify(caps: &Caps) -> Role {
    let has = |k: KeyCode| caps.keys.contains(&k);
    let letters = LETTERS.iter().filter(|&&k| has(k)).count();
    if letters >= 20 && has(KeyCode::KEY_SPACE) {
        Role::Keyboard
    } else if has(KeyCode::BTN_TOOL_PEN) || has(KeyCode::BTN_STYLUS) {
        Role::Tablet
    } else if has(KeyCode::BTN_TOUCH) && caps.direct {
        Role::Touchscreen
    } else if has(KeyCode::BTN_TOUCH) && (caps.pointer || has(KeyCode::BTN_TOOL_FINGER)) {
        Role::Touchpad
    } else if has(KeyCode::BTN_LEFT)
        || has(KeyCode::BTN_RIGHT)
        || (caps.relative && has(KeyCode::BTN_MIDDLE))
    {
        Role::Mouse
    } else {
        Role::Other
    }
}

/// Устойчивое имя устройства для конфига: «vendor:product имя».
pub fn device_key(vendor: u16, product: u16, name: &str) -> String {
    format!("{vendor:04x}:{product:04x} {name}")
}

/// Описание устройства для GUI; полей, которых служба не прислала, — по
/// умолчанию, как в `ipc::Status`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceInfo {
    pub path: String,
    pub name: String,
    pub key: String,
    pub detected: Role,
    pub role: Role,
    pub overridden: bool,
    pub virtual_device: bool,
    /// Сколько миллисекунд назад было последнее событие.
    pub idle_ms: Option<u64>,
}

struct Rules {
    overrides: HashMap<String, Role>,
    ignore_names: Vec<String>,
}

impl Rules {
    fn from_config(cfg: &Config) -> Rules {
        Rules {
            overrides: cfg
                .devices
                .iter()
                .filter_map(|(k, v)| Some((k.clone(), Role::parse(v)?)))
                .collect(),
            ignore_names: cfg.ignore_devices.clone(),
        }
    }

    /// (роль, задана ли вручную)
    fn role(&self, key: &str, name: &str, detected: Role) -> (Role, bool) {
        if name == OWN_KEYBOARD {
            return (Role::Other, false); // собственную виртуальную клавиатуру не слушаем никогда
        }
        if let Some(&role) = self.overrides.get(key) {
            return (role, true);
        }
        if detected != Role::Other && self.ignore_names.iter().any(|n| n == name) {
            return (Role::Ignore, false);
        }
        (detected, false)
    }
}

/// Описать устройство, не подписываясь на его события (для `shiftfix --check`).
pub fn describe(path: &Path, cfg: &Config) -> Option<DeviceInfo> {
    let dev = RawDevice::open(path).ok()?;
    let name = dev.name().unwrap_or_default().to_owned();
    let id = dev.input_id();
    let key = device_key(id.vendor(), id.product(), &name);
    let detected = if name == OWN_KEYBOARD {
        Role::Other
    } else {
        classify(&caps_of(&dev))
    };
    let (role, overridden) = Rules::from_config(cfg).role(&key, &name, detected);
    Some(DeviceInfo {
        path: path.display().to_string(),
        name,
        key,
        detected,
        role,
        overridden,
        virtual_device: id.bus_type().0 == 0x06 || is_virtual(path),
        idle_ms: None,
    })
}

fn caps_of(dev: &RawDevice) -> Caps {
    let props = dev.properties();
    Caps {
        keys: dev
            .supported_keys()
            .map(|keys| keys.iter().collect())
            .unwrap_or_default(),
        relative: dev
            .supported_relative_axes()
            .is_some_and(|axes| axes.contains(RelativeAxisCode::REL_X)),
        direct: props.contains(PropType::DIRECT),
        pointer: props.contains(PropType::POINTER),
    }
}

fn is_virtual(path: &Path) -> bool {
    let Some(node) = path.file_name() else {
        return false;
    };
    let sys = Path::new("/sys/class/input").join(node).join("device");
    fs::canonicalize(sys).is_ok_and(|real| real.to_string_lossy().contains("/devices/virtual/"))
}

fn node_number(path: &Path) -> Option<u32> {
    path.file_name()?
        .to_str()?
        .strip_prefix("event")?
        .parse()
        .ok()
}

pub fn event_nodes() -> Vec<PathBuf> {
    let mut nodes: Vec<PathBuf> = fs::read_dir("/dev/input")
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|p| node_number(p).is_some())
        .collect();
    nodes.sort_by_key(|p| node_number(p));
    nodes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(keys: &[KeyCode], relative: bool, direct: bool, pointer: bool) -> Caps {
        Caps {
            keys: keys.iter().copied().collect(),
            relative,
            direct,
            pointer,
        }
    }

    fn keyboard_keys() -> Vec<KeyCode> {
        let mut keys = LETTERS.to_vec();
        keys.push(KeyCode::KEY_SPACE);
        keys
    }

    #[test]
    fn classifies_typical_devices() {
        assert_eq!(
            classify(&caps(&keyboard_keys(), false, false, false)),
            Role::Keyboard
        );
        let mouse = [KeyCode::BTN_LEFT, KeyCode::BTN_RIGHT, KeyCode::BTN_MIDDLE];
        assert_eq!(classify(&caps(&mouse, true, false, false)), Role::Mouse);
        let pad = [
            KeyCode::BTN_LEFT,
            KeyCode::BTN_TOUCH,
            KeyCode::BTN_TOOL_FINGER,
            KeyCode::BTN_TOOL_DOUBLETAP,
        ];
        assert_eq!(classify(&caps(&pad, false, false, true)), Role::Touchpad);
        let screen = [KeyCode::BTN_TOUCH];
        assert_eq!(
            classify(&caps(&screen, false, true, false)),
            Role::Touchscreen
        );
        let pen = [
            KeyCode::BTN_TOOL_PEN,
            KeyCode::BTN_TOUCH,
            KeyCode::BTN_STYLUS,
        ];
        assert_eq!(classify(&caps(&pen, false, true, false)), Role::Tablet);
        // «Video Bus» и кнопка питания: клавиши есть, но это не клавиатура.
        let video = [
            KeyCode::KEY_BRIGHTNESSUP,
            KeyCode::KEY_BRIGHTNESSDOWN,
            KeyCode::KEY_SWITCHVIDEOMODE,
        ];
        assert_eq!(classify(&caps(&video, false, false, false)), Role::Other);
        assert_eq!(
            classify(&caps(&[KeyCode::KEY_POWER], false, false, false)),
            Role::Other
        );
        // Мультимедийный узел клавиатуры с парой букв — тоже не клавиатура.
        assert_eq!(
            classify(&caps(
                &[KeyCode::KEY_A, KeyCode::KEY_SPACE],
                false,
                false,
                false
            )),
            Role::Other
        );
        // Клавиатура со встроенным тачпадом остаётся клавиатурой.
        let mut combo = keyboard_keys();
        combo.extend([KeyCode::BTN_TOUCH, KeyCode::BTN_TOOL_FINGER]);
        assert_eq!(classify(&caps(&combo, false, false, true)), Role::Keyboard);
    }

    #[test]
    fn rules_prefer_manual_roles() {
        let mut cfg = Config::default();
        cfg.devices
            .insert("1234:5678 Panel".into(), "touchscreen".into());
        let rules = Rules::from_config(&cfg);
        assert_eq!(
            rules.role("1234:5678 Panel", "Panel", Role::Touchpad),
            (Role::Touchscreen, true)
        );
        assert_eq!(
            rules.role(
                "0:0 ydotoold virtual device",
                "ydotoold virtual device",
                Role::Keyboard
            ),
            (Role::Ignore, false)
        );
        assert_eq!(
            rules.role("x", OWN_KEYBOARD, Role::Keyboard),
            (Role::Other, false)
        );
        assert_eq!(
            rules.role("1:1 Power Button", "Power Button", Role::Other),
            (Role::Other, false)
        );
    }

    #[test]
    fn role_names_round_trip() {
        for role in Role::CHOICES {
            assert_eq!(Role::parse(role.as_str()), Some(role));
            assert_eq!(Role::from_u8(role.to_u8()), role);
        }
        assert_eq!(Role::parse("other"), None);
        assert_eq!(device_key(0x46d, 0xc52b, "Logitech"), "046d:c52b Logitech");
    }
}
