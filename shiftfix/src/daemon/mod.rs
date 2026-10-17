//! Служба: потоки ввода, IPC, трей и главный цикл с движком.
//!
//! Все источники событий (evdev, PRIMARY, keymap, сигналы, GUI) шлют сообщения
//! в один канал; движок обрабатывает их по очереди в главном потоке.

mod signals;
mod tray;
pub mod uinput;

use std::fs::{self, File};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, LazyLock};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use evdev::KeyCode;
use log::{error, info, warn};

use crate::config::Config;
use crate::devices::Registry;
use crate::engine::Engine;
use crate::ipc::{Request, Response, Status};
use crate::keymap::{self, Keymap};
use crate::selection::{self, Active};
use crate::sys::session::{self, Logind};
use crate::sys::{env, proc};
use crate::{backend, ipc, logbuf};
use tray::TrayIcon;
use uinput::Injector;

/// Событие для главного цикла службы.
pub enum Msg {
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
    DeviceGone {
        name: String,
    },
    /// PRIMARY изменился; размер нового выделения в байтах.
    Primary(usize),
    /// Слежение за PRIMARY: `Ok(чем)` — работает, иначе причина, почему нет.
    Selection(Result<&'static str, String>),
    Keymap(Keymap),
    TogglePause,
    Reload,
    Quit,
    Ipc(Request, Sender<Response>),
}

static START: LazyLock<Instant> = LazyLock::new(Instant::now);

fn monotonic() -> f64 {
    START.elapsed().as_secs_f64()
}

/// Два экземпляра исправляли бы одно слово дважды.
pub fn lock_file(path: &Path) -> Result<File> {
    let file = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(path)
        .with_context(|| format!("не открывается {}", path.display()))?;
    // SAFETY: flock на собственном открытом дескрипторе.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        bail!("shiftfix уже запущен (если это служба: systemctl --user stop shiftfix)");
    }
    Ok(file)
}

fn notify(enabled: bool, text: &str) {
    if enabled && env::in_path("notify-send") {
        let _ = proc::spawn_detached(
            "notify-send",
            &[
                "-a", "shiftfix", "-i", "shiftfix", "-t", "1500", "shiftfix", text,
            ],
        );
    }
}

struct Daemon {
    path: PathBuf,
    cfg: Config,
    engine: Engine,
    registry: Arc<Registry>,
    tray: Option<TrayIcon>,
    tx: Sender<Msg>,
    selection_note: Option<String>,
    /// Через что сейчас следим за выделением.
    selection_via: Option<&'static str>,
    selection_active: Active,
    watching_primary: bool,
}

pub fn run(path: PathBuf, cfg: Config) -> Result<()> {
    let _lock = lock_file(&env::runtime_dir().join("shiftfix.lock"))?;
    let injector = Injector::new(cfg.key_delay)?;
    let (tx, rx) = mpsc::channel();

    let registry = Registry::new(&cfg, tx.clone());
    let mut stats = registry.scan();
    if registry.keyboard_count() == 0 && stats.denied > 0 {
        // Узел, который udev только что создал, бывает недоступен долю секунды.
        thread::sleep(Duration::from_secs(1));
        stats = registry.scan();
    }
    if registry.keyboard_count() == 0 {
        // Часть устройств закрыта — клавиатуры, скорее всего, среди них. Открыться
        // могут и чужие (джойстики systemd отдаёт пользователю сам), поэтому
        // смотрим не на opened, а на denied.
        if stats.denied > 0 {
            bail!(
                "нет доступа к {} устройствам в /dev/input — добавь себя в группу input \
                 (sudo usermod -aG input $USER) и перезайди в систему",
                stats.denied
            );
        }
        // Bluetooth- и беспроводные клавиатуры подключаются и после входа.
        warn!("клавиатур пока нет — жду подключения");
    }
    let scanner = Arc::clone(&registry);
    thread::Builder::new()
        .name("hotplug".into())
        .spawn(move || {
            loop {
                thread::sleep(Duration::from_secs(2));
                scanner.scan();
            }
        })?;
    signals::spawn(tx.clone())?;
    ipc::serve(tx.clone())?;
    let keymap_tx = tx.clone();
    keymap::spawn_watcher(move |keymap| {
        let _ = keymap_tx.send(Msg::Keymap(keymap));
    });

    let leds = Arc::clone(&registry);
    let selection_active = Active::default();
    let mut logind = Logind::default();
    let engine = Engine::new(
        cfg.clone(),
        backend::make(&cfg),
        Box::new(injector),
        selection::make(&cfg, &selection_active),
        Box::new(move || leds.caps_led()),
        Box::new(monotonic),
        Box::new(move || session::blocked(&mut logind)),
    );
    let mut daemon = Daemon {
        path,
        tray: None,
        engine,
        registry,
        tx,
        selection_note: Some("запускается".into()),
        selection_via: None,
        selection_active,
        watching_primary: false,
        cfg,
    };
    daemon.apply_side_effects();
    let layouts = daemon.engine.layout_info();
    info!(
        "готов: раскладки {} ({}), выделение {}",
        layouts.codes.join(", "),
        layouts.backend,
        if daemon.engine.selection_enabled() {
            "исправляется"
        } else {
            "выключено"
        }
    );
    daemon.update_tray(&layouts.codes);

    for msg in rx {
        if !daemon.handle(msg) {
            break;
        }
    }
    info!("выход");
    let _ = fs::remove_file(ipc::socket_path());
    Ok(())
}

impl Daemon {
    /// Трей и слежение за PRIMARY — по текущему конфигу.
    fn apply_side_effects(&mut self) {
        if self.cfg.tray && self.tray.is_none() {
            self.tray = TrayIcon::spawn(self.tx.clone(), self.engine.enabled());
        } else if !self.cfg.tray {
            self.tray = None;
        }
        if self.cfg.selection && !self.watching_primary {
            selection::spawn_watcher(self.tx.clone(), self.selection_active.clone());
            self.watching_primary = true;
        }
    }

    fn update_tray(&self, codes: &[String]) {
        if let Some(tray) = &self.tray {
            tray.update(self.engine.enabled(), &codes.join(" · "));
        }
    }

    fn set_enabled(&mut self, enabled: bool) {
        if self.engine.enabled() == enabled {
            return;
        }
        self.engine.set_enabled(enabled);
        let state = if enabled {
            "работает"
        } else {
            "на паузе"
        };
        info!("shiftfix {state}");
        notify(self.cfg.notify, state);
        let codes = self.engine.layout_info().codes;
        self.update_tray(&codes);
    }

    fn reload(&mut self) -> Result<()> {
        let cfg = Config::load(&self.path)?;
        let backend_changed = cfg.backend != self.cfg.backend
            || cfg.switch_hotkey != self.cfg.switch_hotkey
            || cfg.hotkey_layouts != self.cfg.hotkey_layouts;
        if backend_changed {
            self.engine.set_backend(backend::make(&cfg));
        }
        self.engine
            .set_clip(selection::make(&cfg, &self.selection_active));
        self.engine.set_config(cfg.clone());
        self.registry.apply(&cfg);
        self.cfg = cfg;
        self.apply_side_effects();
        info!("конфиг перечитан");
        Ok(())
    }

    fn status(&mut self) -> Status {
        let layouts = self.engine.layout_info();
        Status {
            version: env!("CARGO_PKG_VERSION").into(),
            pid: std::process::id(),
            uptime_secs: START.elapsed().as_secs(),
            enabled: self.engine.enabled(),
            layouts,
            selection: self.engine.selection_enabled() && self.selection_note.is_none(),
            selection_note: if self.engine.selection_enabled() {
                self.selection_note.clone()
            } else {
                Some("выключено в настройках".into())
            },
            selection_via: self
                .selection_via
                .filter(|_| self.engine.selection_enabled())
                .map(Into::into),
            tray: self.tray.as_ref().is_some_and(TrayIcon::visible),
            tray_waiting: self.tray.as_ref().is_some_and(|tray| !tray.visible()),
            devices: self.registry.snapshot(),
            actions: self.engine.actions(),
            log: logbuf::recent(),
            config_path: self.path.display().to_string(),
        }
    }

    /// `false` — пора выходить.
    fn handle(&mut self, msg: Msg) -> bool {
        match msg {
            Msg::Key { key, value, ts } => self.engine.key(key, value, ts),
            Msg::Button { value, ts } => self.engine.button(value, ts),
            Msg::Touch { device, value, ts } => self.engine.touch(&device, value, ts),
            Msg::Dropped => self.engine.resync(self.registry.held_keys()),
            Msg::DeviceGone { name } => {
                info!("устройство отключено: {name}");
                self.engine.resync(self.registry.held_keys());
            }
            Msg::Primary(size) => self.engine.primary_changed(size),
            Msg::Selection(state) => {
                let (via, note) = match state {
                    Ok(via) => (Some(via), None),
                    Err(reason) => (None, Some(reason)),
                };
                if via != self.selection_via || note != self.selection_note {
                    match (&via, &note) {
                        (Some(via), _) => info!("слежу за выделением (PRIMARY) через {via}"),
                        (None, Some(reason)) => warn!("выделение не отслеживается: {reason}"),
                        (None, None) => {}
                    }
                    self.selection_via = via;
                    self.selection_note = note;
                }
            }
            Msg::Keymap(keymap) => self.engine.set_keymap(keymap),
            Msg::TogglePause => {
                let enabled = !self.engine.enabled();
                self.set_enabled(enabled);
            }
            Msg::Reload => {
                if let Err(err) = self.reload() {
                    error!("конфиг не перечитан: {err:#}");
                }
            }
            Msg::Quit => return false,
            Msg::Ipc(req, reply) => {
                let quit = req == Request::Quit;
                let response = match req {
                    Request::Status => Response {
                        status: Some(Box::new(self.status())),
                        ..Response::ok()
                    },
                    Request::SetEnabled { enabled } => {
                        self.set_enabled(enabled);
                        Response::ok()
                    }
                    Request::Toggle => {
                        let enabled = !self.engine.enabled();
                        self.set_enabled(enabled);
                        Response::ok()
                    }
                    Request::Reload => match self.reload() {
                        Ok(()) => Response::ok(),
                        Err(err) => Response::error(format!("{err:#}")),
                    },
                    Request::Quit => Response::ok(),
                };
                let _ = reply.send(response);
                if quit {
                    return false;
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Backend, Keys, Stroke};

    struct NullKeys;

    impl Keys for NullKeys {
        fn backspace(&mut self, _: usize) -> Result<()> {
            Ok(())
        }
        fn type_strokes(&mut self, _: &[Stroke]) -> Result<()> {
            Ok(())
        }
        fn combo(&mut self, _: &[KeyCode]) -> Result<()> {
            Ok(())
        }
    }

    struct TwoLayouts(usize);

    impl Backend for TwoLayouts {
        fn name(&self) -> &'static str {
            "тест"
        }
        fn layouts(&mut self) -> Result<Vec<String>> {
            Ok(vec!["us".into(), "ru".into()])
        }
        fn current(&mut self) -> Result<Option<usize>> {
            Ok(Some(self.0))
        }
        fn set(&mut self, index: usize, _: &mut dyn Keys) -> Result<()> {
            self.0 = index;
            Ok(())
        }
        fn toggle(&mut self, _: &mut dyn Keys) -> Result<()> {
            self.0 ^= 1;
            Ok(())
        }
    }

    fn daemon(path: PathBuf) -> Daemon {
        let cfg = Config {
            tray: false,
            selection: false,
            notify: false,
            ..Config::default()
        };
        let (tx, _rx) = mpsc::channel();
        let registry = Registry::new(&cfg, tx.clone());
        let engine = Engine::new(
            cfg.clone(),
            Box::new(TwoLayouts(0)),
            Box::new(NullKeys),
            None,
            Box::new(|| false),
            Box::new(monotonic),
            Box::new(|| None),
        );
        Daemon {
            path,
            cfg,
            engine,
            registry,
            tray: None,
            tx,
            selection_note: None,
            selection_via: None,
            selection_active: Active::default(),
            watching_primary: false,
        }
    }

    fn ask(d: &mut Daemon, req: Request) -> (bool, Response) {
        let (reply, answer) = mpsc::channel();
        let alive = d.handle(Msg::Ipc(req, reply));
        (alive, answer.recv().unwrap())
    }

    #[test]
    fn answers_gui_requests() {
        let dir = std::env::temp_dir().join(format!("shiftfix-daemon-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("config.toml");
        let mut d = daemon(path.clone());

        let (_, resp) = ask(&mut d, Request::Status);
        let status = resp.status.unwrap();
        assert!(status.enabled);
        assert_eq!(status.layouts.codes, ["us", "ru"]);
        assert_eq!(status.layouts.current, Some(0));
        assert_eq!(status.layouts.backend, "тест");
        assert!(!status.selection && status.selection_note.is_some());

        assert!(ask(&mut d, Request::Toggle).1.ok);
        assert!(!ask(&mut d, Request::Status).1.status.unwrap().enabled);
        assert!(ask(&mut d, Request::SetEnabled { enabled: true }).1.ok);
        assert!(ask(&mut d, Request::Status).1.status.unwrap().enabled);

        // Перечитать конфиг: нет файла — значения по умолчанию, плохой файл — ошибка.
        assert!(ask(&mut d, Request::Reload).1.ok);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            &path,
            "tray = false\nselection = false\ndouble_tap_ms = 250\n",
        )
        .unwrap();
        assert!(ask(&mut d, Request::Reload).1.ok);
        assert_eq!(d.cfg.double_tap, 0.25);
        fs::write(&path, "backend = \"windows\"\n").unwrap();
        let (_, resp) = ask(&mut d, Request::Reload);
        assert!(!resp.ok && resp.error.unwrap().contains("backend"));

        let (alive, resp) = ask(&mut d, Request::Quit);
        assert!(!alive && resp.ok);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn keyboard_events_reach_engine() {
        let mut d = daemon(std::env::temp_dir().join("shiftfix-none.toml"));
        for (key, value, ts) in [
            (KeyCode::KEY_G, 1, 1.00),
            (KeyCode::KEY_G, 0, 1.05),
            (KeyCode::KEY_LEFTSHIFT, 1, 1.20),
            (KeyCode::KEY_LEFTSHIFT, 0, 1.25),
            (KeyCode::KEY_LEFTSHIFT, 1, 1.35),
            (KeyCode::KEY_LEFTSHIFT, 0, 1.40),
        ] {
            assert!(d.handle(Msg::Key { key, value, ts }));
        }
        let status = ask(&mut d, Request::Status).1.status.unwrap();
        assert_eq!(
            status.layouts.current,
            Some(1),
            "двойной Shift переключил раскладку"
        );
        assert_eq!(status.actions.len(), 1);
        assert_eq!(
            (
                status.actions[0].from.as_str(),
                status.actions[0].to.as_str()
            ),
            ("us", "ru")
        );
    }

    #[test]
    fn second_instance_is_refused() {
        let path = std::env::temp_dir().join(format!("shiftfix-lock-{}", std::process::id()));
        let first = lock_file(&path).unwrap();
        assert!(lock_file(&path).is_err());
        drop(first);
        assert!(lock_file(&path).is_ok());
        fs::remove_file(path).unwrap();
    }
}
