//! Устройства ввода: что это (клавиатура, мышь, тачпад, сенсорный экран, перо),
//! кого слушать и как понимать их события.
//!
//! Роль определяется по возможностям устройства (свойства INPUT_PROP_DIRECT /
//! POINTER, кнопки, оси). Если автоматика ошиблась — роль можно задать вручную
//! в GUI, она хранится в `[devices]` конфига.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Instant, UNIX_EPOCH};

use evdev::raw_stream::RawDevice;
use evdev::{
    EventType, InputEvent, KeyCode, LedCode, PropType, RelativeAxisCode, SynchronizationCode,
};
use log::{info, warn};
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::daemon::Msg;
use crate::daemon::uinput;
use crate::keys::{is_button, is_click};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
        [
            Role::Keyboard,
            Role::Mouse,
            Role::Touchpad,
            Role::Touchscreen,
            Role::Tablet,
            Role::Ignore,
            Role::Other,
        ]
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

/// Описание устройства для GUI.
#[derive(Clone, Debug, Serialize, Deserialize)]
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

struct Shared {
    role: AtomicU8,
    /// Мс от старта реестра до последнего события; 0 — событий не было.
    last_event: AtomicU64,
    alive: AtomicBool,
}

struct Entry {
    name: String,
    key: String,
    detected: Role,
    overridden: bool,
    virtual_device: bool,
    stamp: (i64, i64),
    reading: bool,
    shared: Arc<Shared>,
}

impl Entry {
    /// Запись всё ещё описывает устройство на этом узле.
    fn still_valid(&self, path: &Path) -> bool {
        if self.reading {
            // Отключение поток чтения замечает сам (ENODEV). По ctime тут судить
            // нельзя: udev обновляет время узла на каждом событии change
            // (`udevadm trigger`), и второй поток слал бы каждое событие повторно.
            self.shared.alive.load(Ordering::Relaxed)
        } else {
            // Без потока замену устройства на том же узле видно только по ctime.
            fs::metadata(path).is_ok_and(|m| (m.ctime(), m.ctime_nsec()) == self.stamp)
        }
    }
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
        if name == uinput::NAME {
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

/// Итог обхода /dev/input: сколько устройств открылось и сколько — нет прав.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScanStats {
    pub opened: usize,
    pub denied: usize,
}

/// Все устройства ввода и потоки их чтения.
pub struct Registry {
    entries: Mutex<HashMap<PathBuf, Entry>>,
    /// Проходы из потока hotplug и из apply() не пересекаются: иначе оба увидят,
    /// что записи нет, и оба откроют устройство.
    scanning: Mutex<()>,
    rules: Mutex<Rules>,
    start: Instant,
    tx: Sender<Msg>,
}

impl Registry {
    pub fn new(cfg: &Config, tx: Sender<Msg>) -> Arc<Registry> {
        Arc::new(Registry {
            entries: Mutex::default(),
            scanning: Mutex::default(),
            rules: Mutex::new(Rules::from_config(cfg)),
            start: Instant::now(),
            tx,
        })
    }

    fn now_ms(&self) -> u64 {
        u64::try_from(self.start.elapsed().as_millis())
            .unwrap_or(u64::MAX)
            .max(1)
    }

    /// Найти новые устройства, забыть отключённые.
    pub fn scan(self: &Arc<Self>) -> ScanStats {
        let _scanning = self.scanning.lock().expect("scan");
        let nodes = event_nodes();
        self.entries
            .lock()
            .expect("devices")
            .retain(|path, e| e.still_valid(path));
        let mut stats = ScanStats::default();
        for path in nodes {
            if self.entries.lock().expect("devices").contains_key(&path) {
                stats.opened += 1;
                continue;
            }
            let Ok(meta) = fs::metadata(&path) else {
                continue;
            };
            match RawDevice::open(&path) {
                Ok(dev) => {
                    stats.opened += 1;
                    self.add(path, (meta.ctime(), meta.ctime_nsec()), dev);
                }
                // Нет прав (или udev ещё не выставил группу новому узлу) — попробуем позже.
                Err(err) if err.kind() == ErrorKind::PermissionDenied => stats.denied += 1,
                Err(_) => {}
            }
        }
        stats
    }

    fn add(self: &Arc<Self>, path: PathBuf, stamp: (i64, i64), dev: RawDevice) {
        let name = dev.name().unwrap_or_default().to_owned();
        let id = dev.input_id();
        let key = device_key(id.vendor(), id.product(), &name);
        let detected = if name == uinput::NAME {
            Role::Other
        } else {
            classify(&caps_of(&dev))
        };
        let (role, overridden) = self
            .rules
            .lock()
            .expect("rules")
            .role(&key, &name, detected);
        let virtual_device = id.bus_type().0 == 0x06 || is_virtual(&path);
        let shared = Arc::new(Shared {
            role: AtomicU8::new(role.to_u8()),
            last_event: AtomicU64::new(0),
            alive: AtomicBool::new(true),
        });
        // Устройства «Другое» не открываем вовсе: датчики могут слать события постоянно.
        let reading = role != Role::Other;
        if reading {
            if role.listens() {
                info!("слушаю ({}): {name}", role.title().to_lowercase());
            }
            self.spawn_reader(path.clone(), name.clone(), dev, shared.clone());
        }
        let entry = Entry {
            name,
            key,
            detected,
            overridden,
            virtual_device,
            stamp,
            reading,
            shared,
        };
        self.entries.lock().expect("devices").insert(path, entry);
    }

    fn spawn_reader(
        self: &Arc<Self>,
        path: PathBuf,
        name: String,
        mut dev: RawDevice,
        shared: Arc<Shared>,
    ) {
        let registry = Arc::clone(self);
        let tag: Arc<str> = path.to_string_lossy().into();
        let spawned = thread::Builder::new()
            .name(format!("evdev {}", path.display()))
            .spawn(move || {
                'read: loop {
                    let events = match dev.fetch_events() {
                        Ok(events) => events,
                        Err(err) if err.kind() == ErrorKind::Interrupted => continue,
                        Err(_) => break, // устройство отключено
                    };
                    for event in events {
                        shared
                            .last_event
                            .store(registry.now_ms(), Ordering::Relaxed);
                        let role = Role::from_u8(shared.role.load(Ordering::Relaxed));
                        if let Some(msg) = translate(role, &tag, event)
                            && registry.tx.send(msg).is_err()
                        {
                            break 'read;
                        }
                    }
                }
                shared.alive.store(false, Ordering::Relaxed);
                let _ = registry.tx.send(Msg::DeviceGone { name });
            });
        if let Err(err) = spawned {
            warn!("поток чтения {} не запустился: {err}", path.display());
        }
    }

    /// Новые правила из конфига: роли меняются на лету, «Другое» → роль открывается заново.
    pub fn apply(self: &Arc<Self>, cfg: &Config) {
        *self.rules.lock().expect("rules") = Rules::from_config(cfg);
        let mut reopen = Vec::new();
        {
            let rules = self.rules.lock().expect("rules");
            let mut entries = self.entries.lock().expect("devices");
            for (path, e) in entries.iter_mut() {
                let (role, overridden) = rules.role(&e.key, &e.name, e.detected);
                e.overridden = overridden;
                e.shared.role.store(role.to_u8(), Ordering::Relaxed);
                if role != Role::Other && !e.reading {
                    reopen.push(path.clone());
                }
            }
        }
        if !reopen.is_empty() {
            self.entries
                .lock()
                .expect("devices")
                .retain(|p, _| !reopen.contains(p));
            self.scan();
        }
    }

    pub fn snapshot(&self) -> Vec<DeviceInfo> {
        let now = self.now_ms();
        let entries = self.entries.lock().expect("devices");
        let mut list: Vec<DeviceInfo> = entries
            .iter()
            .map(|(path, e)| {
                let last = e.shared.last_event.load(Ordering::Relaxed);
                DeviceInfo {
                    path: path.display().to_string(),
                    name: e.name.clone(),
                    key: e.key.clone(),
                    detected: e.detected,
                    role: Role::from_u8(e.shared.role.load(Ordering::Relaxed)),
                    overridden: e.overridden,
                    virtual_device: e.virtual_device,
                    idle_ms: (last > 0).then(|| now.saturating_sub(last)),
                }
            })
            .collect();
        list.sort_by_key(|d| node_number(Path::new(&d.path)));
        list
    }

    fn with_role(&self, role: Role) -> Vec<PathBuf> {
        let entries = self.entries.lock().expect("devices");
        entries
            .iter()
            .filter(|(_, e)| {
                e.reading && Role::from_u8(e.shared.role.load(Ordering::Relaxed)) == role
            })
            .map(|(p, _)| p.clone())
            .collect()
    }

    pub fn keyboard_count(&self) -> usize {
        self.with_role(Role::Keyboard).len()
    }

    /// Клавиши, зажатые сейчас (по данным ядра).
    pub fn held_keys(&self) -> HashSet<KeyCode> {
        self.with_role(Role::Keyboard)
            .iter()
            .filter_map(|p| RawDevice::open(p).and_then(|d| d.get_key_state()).ok())
            .flat_map(|state| state.iter().filter(|&k| !is_button(k)).collect::<Vec<_>>())
            .collect()
    }

    pub fn caps_led(&self) -> bool {
        self.with_role(Role::Keyboard).iter().any(|p| {
            RawDevice::open(p)
                .and_then(|d| d.get_led_state())
                .is_ok_and(|leds| leds.contains(LedCode::LED_CAPSL))
        })
    }
}

/// Описать устройство, не подписываясь на его события (для `shiftfix --check`).
pub fn describe(path: &Path, cfg: &Config) -> Option<DeviceInfo> {
    let dev = RawDevice::open(path).ok()?;
    let name = dev.name().unwrap_or_default().to_owned();
    let id = dev.input_id();
    let key = device_key(id.vendor(), id.product(), &name);
    let detected = if name == uinput::NAME {
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

/// Перевод события в сообщение движку с учётом роли устройства.
pub fn translate(role: Role, tag: &Arc<str>, event: InputEvent) -> Option<Msg> {
    if !role.listens() {
        return None;
    }
    let ts = event
        .timestamp()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64());
    if event.event_type() == EventType::KEY {
        let key = KeyCode::new(event.code());
        let value = event.value();
        return if is_click(key) {
            Some(Msg::Button { value, ts })
        } else if key == KeyCode::BTN_TOUCH {
            match role {
                // Касание экрана или пера — всегда клик: курсор и фокус могли сместиться.
                Role::Touchscreen | Role::Tablet => {
                    (value == 1).then_some(Msg::Button { value: 1, ts })
                }
                // На тачпаде кликом считается только короткий тап.
                _ => Some(Msg::Touch {
                    device: tag.clone(),
                    value,
                    ts,
                }),
            }
        } else if is_button(key) {
            None
        } else {
            Some(Msg::Key { key, value, ts })
        };
    }
    let dropped = event.event_type() == EventType::SYNCHRONIZATION
        && event.code() == SynchronizationCode::SYN_DROPPED.0;
    dropped.then_some(Msg::Dropped)
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

    fn entry(stamp: (i64, i64), reading: bool, alive: bool) -> Entry {
        Entry {
            name: "test".into(),
            key: "0000:0000 test".into(),
            detected: Role::Keyboard,
            overridden: false,
            virtual_device: false,
            stamp,
            reading,
            shared: Arc::new(Shared {
                role: AtomicU8::new(Role::Keyboard.to_u8()),
                last_event: AtomicU64::new(0),
                alive: AtomicBool::new(alive),
            }),
        }
    }

    fn stamp(path: &Path) -> (i64, i64) {
        let meta = fs::metadata(path).unwrap();
        (meta.ctime(), meta.ctime_nsec())
    }

    #[test]
    fn live_reader_survives_udev_touching_node() {
        let path = std::env::temp_dir().join(format!("shiftfix-node-{}", std::process::id()));
        fs::write(&path, b"").unwrap();
        let before = stamp(&path);
        // Так udev обновляет время узла на событии change: ctime меняется,
        // а устройство то же самое.
        thread::sleep(std::time::Duration::from_millis(20));
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(std::time::SystemTime::now()))
            .unwrap();
        assert_ne!(stamp(&path), before);

        assert!(
            entry(before, true, true).still_valid(&path),
            "поток жив — второй не нужен"
        );
        assert!(
            !entry(before, true, false).still_valid(&path),
            "поток завершился — открыть заново"
        );
        assert!(
            !entry(before, false, true).still_valid(&path),
            "без потока узел сменился по ctime"
        );
        assert!(entry(stamp(&path), false, true).still_valid(&path));
        fs::remove_file(&path).unwrap();
        assert!(
            !entry(before, false, true).still_valid(&path),
            "узла больше нет"
        );
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
            rules.role("x", uinput::NAME, Role::Keyboard),
            (Role::Other, false)
        );
        assert_eq!(
            rules.role("1:1 Power Button", "Power Button", Role::Other),
            (Role::Other, false)
        );
    }

    #[test]
    fn touch_means_click_on_screens_only() {
        let tag: Arc<str> = "/dev/input/event7".into();
        let touch = |role, value| {
            translate(
                role,
                &tag,
                InputEvent::new(EventType::KEY.0, KeyCode::BTN_TOUCH.code(), value),
            )
        };
        assert!(matches!(
            touch(Role::Touchscreen, 1),
            Some(Msg::Button { value: 1, .. })
        ));
        assert!(touch(Role::Touchscreen, 0).is_none());
        assert!(matches!(touch(Role::Tablet, 1), Some(Msg::Button { .. })));
        assert!(matches!(
            touch(Role::Touchpad, 1),
            Some(Msg::Touch { value: 1, .. })
        ));
        assert!(touch(Role::Ignore, 1).is_none());
        let key = translate(
            Role::Keyboard,
            &tag,
            InputEvent::new(EventType::KEY.0, KeyCode::KEY_A.code(), 1),
        );
        assert!(matches!(
            key,
            Some(Msg::Key {
                key: KeyCode::KEY_A,
                value: 1,
                ..
            })
        ));
        let pen_button = translate(
            Role::Tablet,
            &tag,
            InputEvent::new(EventType::KEY.0, KeyCode::BTN_STYLUS.code(), 1),
        );
        assert!(pen_button.is_none());
        let dropped = InputEvent::new(
            EventType::SYNCHRONIZATION.0,
            SynchronizationCode::SYN_DROPPED.0,
            0,
        );
        assert!(matches!(
            translate(Role::Mouse, &tag, dropped),
            Some(Msg::Dropped)
        ));
        assert!(
            translate(
                Role::Mouse,
                &tag,
                InputEvent::new(EventType::RELATIVE.0, 0, 5)
            )
            .is_none()
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
