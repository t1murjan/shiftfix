//! Слушать устройства: обход /dev/input, поток чтения на каждое устройство
//! и перевод событий в `Event` для службы.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Instant, UNIX_EPOCH};

use evdev::raw_stream::RawDevice;
use evdev::{EventType, InputEvent, KeyCode, LedCode, SynchronizationCode};
use log::{info, warn};

use super::{DeviceInfo, Identity, Role, Rules, event_nodes, node_number};
use crate::config::Config;
use crate::keys::{is_button, is_click};

struct Shared {
    role: AtomicU8,
    /// Мс от старта реестра до последнего события; 0 — событий не было.
    last_event: AtomicU64,
    alive: AtomicBool,
}

struct Entry {
    id: Identity,
    overridden: bool,
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

/// Событие устройства для движка — уже с учётом роли устройства.
pub enum Event {
    Key {
        key: KeyCode,
        value: i32,
        ts: f64,
    },
    Button {
        value: i32,
        ts: f64,
    },
    Touch {
        device: Arc<str>,
        value: i32,
        ts: f64,
    },
    /// Ядро потеряло события (SYN_DROPPED).
    Dropped,
    Gone {
        name: String,
    },
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
    /// Проход из потока hotplug и apply() не пересекаются: иначе оба увидят,
    /// что записи нет, и оба откроют устройство, или новое устройство получит
    /// роль по прежним правилам уже после того, как apply() их сменил.
    scanning: Mutex<()>,
    rules: Mutex<Rules>,
    start: Instant,
    /// Куда отдавать события; `false` — получателя больше нет.
    send: Box<dyn Fn(Event) -> bool + Send + Sync>,
}

impl Registry {
    pub fn new(
        cfg: &Config,
        send: impl Fn(Event) -> bool + Send + Sync + 'static,
    ) -> Arc<Registry> {
        Arc::new(Registry {
            entries: Mutex::default(),
            scanning: Mutex::default(),
            rules: Mutex::new(Rules::from_config(cfg)),
            start: Instant::now(),
            send: Box::new(send),
        })
    }

    fn entries(&self) -> MutexGuard<'_, HashMap<PathBuf, Entry>> {
        self.entries.lock().expect("devices")
    }

    fn rules(&self) -> MutexGuard<'_, Rules> {
        self.rules.lock().expect("rules")
    }

    fn now_ms(&self) -> u64 {
        u64::try_from(self.start.elapsed().as_millis())
            .unwrap_or(u64::MAX)
            .max(1)
    }

    /// Найти новые устройства, забыть отключённые.
    pub fn scan(self: &Arc<Self>) -> ScanStats {
        let _scanning = self.scanning.lock().expect("scan");
        self.scan_locked()
    }

    /// Проход по /dev/input; `scanning` уже взят.
    fn scan_locked(self: &Arc<Self>) -> ScanStats {
        let nodes = event_nodes();
        self.entries().retain(|path, e| e.still_valid(path));
        let mut stats = ScanStats::default();
        for path in nodes {
            if self.entries().contains_key(&path) {
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
        let id = Identity::of(&path, &dev);
        let (role, overridden) = id.role(&self.rules());
        let shared = Arc::new(Shared {
            role: AtomicU8::new(role.to_u8()),
            last_event: AtomicU64::new(0),
            alive: AtomicBool::new(true),
        });
        // Устройства «Другое» не открываем вовсе: датчики могут слать события постоянно.
        let reading = role != Role::Other;
        if reading {
            if role.listens() {
                info!("слушаю ({}): {}", role.title().to_lowercase(), id.name);
            }
            self.spawn_reader(path.clone(), id.name.clone(), dev, shared.clone());
        }
        let entry = Entry {
            id,
            overridden,
            stamp,
            reading,
            shared,
        };
        self.entries().insert(path, entry);
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
                        if let Some(event) = translate(role, &tag, event)
                            && !(registry.send)(event)
                        {
                            break 'read;
                        }
                    }
                }
                shared.alive.store(false, Ordering::Relaxed);
                (registry.send)(Event::Gone { name });
            });
        if let Err(err) = spawned {
            warn!("поток чтения {} не запустился: {err}", path.display());
        }
    }

    /// Новые правила из конфига: роли меняются на лету, «Другое» → роль открывается заново.
    pub fn apply(self: &Arc<Self>, cfg: &Config) {
        let _scanning = self.scanning.lock().expect("scan");
        let rules = Rules::from_config(cfg);
        let mut reopen = Vec::new();
        {
            let mut entries = self.entries();
            for (path, e) in entries.iter_mut() {
                let (role, overridden) = e.id.role(&rules);
                e.overridden = overridden;
                e.shared.role.store(role.to_u8(), Ordering::Relaxed);
                if role != Role::Other && !e.reading {
                    reopen.push(path.clone());
                }
            }
            entries.retain(|p, _| !reopen.contains(p));
        }
        *self.rules() = rules;
        if !reopen.is_empty() {
            self.scan_locked();
        }
    }

    pub fn snapshot(&self) -> Vec<DeviceInfo> {
        let now = self.now_ms();
        let entries = self.entries();
        let mut list: Vec<DeviceInfo> = entries
            .iter()
            .map(|(path, e)| {
                let last = e.shared.last_event.load(Ordering::Relaxed);
                DeviceInfo {
                    path: path.display().to_string(),
                    name: e.id.name.clone(),
                    key: e.id.key.clone(),
                    detected: e.id.detected,
                    role: Role::from_u8(e.shared.role.load(Ordering::Relaxed)),
                    overridden: e.overridden,
                    virtual_device: e.id.virtual_device,
                    idle_ms: (last > 0).then(|| now.saturating_sub(last)),
                }
            })
            .collect();
        list.sort_by_key(|d| node_number(Path::new(&d.path)));
        list
    }

    fn with_role(&self, role: Role) -> Vec<PathBuf> {
        self.entries()
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

/// Перевод события evdev для движка с учётом роли устройства.
pub fn translate(role: Role, tag: &Arc<str>, event: InputEvent) -> Option<Event> {
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
            Some(Event::Button { value, ts })
        } else if key == KeyCode::BTN_TOUCH {
            match role {
                // Касание экрана или пера — всегда клик: курсор и фокус могли сместиться.
                Role::Touchscreen | Role::Tablet => {
                    (value == 1).then_some(Event::Button { value: 1, ts })
                }
                // На тачпаде кликом считается только короткий тап.
                _ => Some(Event::Touch {
                    device: tag.clone(),
                    value,
                    ts,
                }),
            }
        } else if is_button(key) {
            None
        } else {
            Some(Event::Key { key, value, ts })
        };
    }
    let dropped = event.event_type() == EventType::SYNCHRONIZATION
        && event.code() == SynchronizationCode::SYN_DROPPED.0;
    dropped.then_some(Event::Dropped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(stamp: (i64, i64), reading: bool, alive: bool) -> Entry {
        Entry {
            id: Identity {
                name: "test".into(),
                key: "0000:0000 test".into(),
                detected: Role::Keyboard,
                virtual_device: false,
            },
            overridden: false,
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
        let dir = crate::testutil::TempDir::new("node");
        let path = dir.join("event0");
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
    fn new_rules_wait_for_scan_in_progress() {
        let registry = Registry::new(&Config::default(), |_| true);
        let mut cfg = Config::default();
        cfg.devices.insert("1:1 Pad".into(), "ignore".into());
        let has_new_rules =
            |r: &Registry| r.rules.lock().unwrap().overrides.contains_key("1:1 Pad");

        // Пока идёт обход, новое устройство получает роль по текущим правилам —
        // apply() должен дождаться его конца и пройти и по нему.
        let scanning = registry.scanning.lock().unwrap();
        let applying = thread::spawn({
            let registry = Arc::clone(&registry);
            move || registry.apply(&cfg)
        });
        thread::sleep(std::time::Duration::from_millis(100));
        assert!(
            !has_new_rules(&registry),
            "правила сменились посреди обхода"
        );
        drop(scanning);
        applying.join().unwrap();
        assert!(has_new_rules(&registry));
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
            Some(Event::Button { value: 1, .. })
        ));
        assert!(touch(Role::Touchscreen, 0).is_none());
        assert!(matches!(touch(Role::Tablet, 1), Some(Event::Button { .. })));
        assert!(matches!(
            touch(Role::Touchpad, 1),
            Some(Event::Touch { value: 1, .. })
        ));
        assert!(touch(Role::Ignore, 1).is_none());
        let key = translate(
            Role::Keyboard,
            &tag,
            InputEvent::new(EventType::KEY.0, KeyCode::KEY_A.code(), 1),
        );
        assert!(matches!(
            key,
            Some(Event::Key {
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
            Some(Event::Dropped)
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
}
