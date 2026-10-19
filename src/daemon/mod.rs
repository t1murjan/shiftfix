//! Служба: потоки ввода, IPC, трей и главный цикл с движком.
//!
//! Все источники событий (evdev, PRIMARY, keymap, сигналы, сокет IPC, трей)
//! шлют сообщения в один канал; движок обрабатывает их по очереди в главном
//! потоке.

mod signals;
mod socket;
mod tray;
mod uinput;

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, LazyLock};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use log::{debug, error, info, warn};

use crate::config::Config;
use crate::devices::{Event, Registry};
use crate::engine::{Engine, Hooks};
use crate::ipc::{Request, Response, Status};
use crate::keymap::{self, Keymap};
use crate::selection::{self, Active};
use crate::sys::session::{self, Logind};
use crate::sys::{lock, paths, proc};
use crate::{backend, ipc, logbuf};
use tray::TrayIcon;
use uinput::Injector;

/// Событие для главного цикла службы.
pub(crate) enum Msg {
    /// От устройств ввода (`devices`).
    Input(Event),
    /// От слежения за выделением (`selection`).
    Selection(selection::Event),
    Keymap(Keymap),
    TogglePause,
    Reload,
    Quit,
    /// Запрос из сокета. После `deadline` главный цикл его не выполняет и
    /// бросает `reply` — сервер тут же отвечает клиенту «служба занята».
    Ipc {
        req: Request,
        deadline: Instant,
        reply: Sender<Response>,
    },
}

static START: LazyLock<Instant> = LazyLock::new(Instant::now);

fn monotonic() -> f64 {
    START.elapsed().as_secs_f64()
}

/// Блокировка одного экземпляра службы. Её же ждёт окно, когда перезапускает
/// службу без systemd.
pub fn lock_path() -> PathBuf {
    paths::runtime_dir().join("shiftfix.lock")
}

/// Два экземпляра исправляли бы одно слово дважды.
pub(crate) fn lock_file(path: &Path) -> Result<File> {
    lock::try_lock(path)
        .with_context(|| format!("не открывается {}", path.display()))?
        .context("shiftfix уже запущен (если это служба: systemctl --user stop shiftfix)")
}

struct Daemon {
    path: PathBuf,
    cfg: Config,
    engine: Engine,
    registry: Arc<Registry>,
    tray: Option<TrayIcon>,
    /// Что сейчас в трее: работает ли служба и раскладки в подсказке.
    tray_shown: Option<(bool, String)>,
    tx: Sender<Msg>,
    /// Слежение за выделением: `Ok(чем)` — работает, иначе причина, почему нет.
    selection: Result<&'static str, String>,
    selection_active: Active,
    selection_watcher: Option<selection::Watcher>,
}

pub fn run(path: PathBuf, cfg: Config) -> Result<()> {
    let _lock = lock_file(&lock_path())?;
    let injector = Injector::new(cfg.key_delay)?;
    let (tx, rx) = mpsc::channel();

    let input_tx = tx.clone();
    let registry = Registry::new(&cfg, move |event| input_tx.send(Msg::Input(event)).is_ok());
    first_scan(&registry)?;
    registry.spawn_hotplug()?;
    signals::spawn(tx.clone())?;
    let _socket = socket::serve(ipc::socket_path(), tx.clone())?;
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
        Hooks {
            caps_on: Box::new(move || leds.caps_led()),
            clock: Box::new(monotonic),
            guard: Box::new(move || session::blocked(&mut logind)),
        },
    );
    let mut daemon = Daemon::new(path, cfg, engine, registry, tx, selection_active);
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
    daemon.update_tray();

    for msg in rx {
        if !daemon.handle(msg) {
            break;
        }
    }
    info!("выход");
    Ok(())
}

/// Первый обход /dev/input. Ошибка — если клавиатуры, скорее всего, есть, но
/// закрыты правами.
fn first_scan(registry: &Arc<Registry>) -> Result<()> {
    let mut denied = registry.scan();
    if registry.keyboard_count() == 0 && denied > 0 {
        // Узел, который udev только что создал, бывает недоступен долю секунды.
        thread::sleep(Duration::from_secs(1));
        denied = registry.scan();
    }
    if registry.keyboard_count() == 0 {
        // Часть устройств закрыта — клавиатуры, скорее всего, среди них. Открыться
        // могут и чужие (джойстики systemd отдаёт пользователю сам), поэтому
        // смотрим не на число открытых устройств, а на закрытые правами.
        if denied > 0 {
            bail!(
                "нет доступа к {} устройствам в /dev/input — добавь себя в группу input \
                 (sudo usermod -aG input $USER) и перезайди в систему",
                denied
            );
        }
        // Bluetooth- и беспроводные клавиатуры подключаются и после входа.
        warn!("клавиатур пока нет — жду подключения");
    }
    Ok(())
}

impl Daemon {
    fn new(
        path: PathBuf,
        cfg: Config,
        engine: Engine,
        registry: Arc<Registry>,
        tx: Sender<Msg>,
        selection_active: Active,
    ) -> Daemon {
        Daemon {
            path,
            cfg,
            engine,
            registry,
            tray: None,
            tray_shown: None,
            tx,
            selection: Err("запускается".into()),
            selection_active,
            selection_watcher: None,
        }
    }

    /// Трей и слежение за PRIMARY — по текущему конфигу.
    fn apply_side_effects(&mut self) {
        if self.cfg.tray && self.tray.is_none() {
            self.tray = TrayIcon::spawn(self.tx.clone(), self.engine.enabled());
            self.tray_shown = None;
        } else if !self.cfg.tray {
            self.tray = None;
            self.tray_shown = None;
        }
        if !self.cfg.selection {
            self.selection_watcher = None;
        } else if self.selection_watcher.is_none() {
            self.selection = Err("запускается".into());
            let tx = self.tx.clone();
            self.selection_watcher = selection::spawn_watcher(
                move |event| tx.send(Msg::Selection(event)).is_ok(),
                self.selection_active.clone(),
            );
        }
    }

    /// Значок и подсказку — по службе: пауза, список раскладок (его меняют
    /// новая keymap и смена переключателя в конфиге).
    fn update_tray(&mut self) {
        if !self.cfg.tray {
            return;
        }
        let shown = (
            self.engine.enabled(),
            self.engine.layout_info().codes.join(" · "),
        );
        if self.tray_shown.as_ref() == Some(&shown) {
            return;
        }
        if let Some(tray) = &self.tray {
            tray.update(shown.0, &shown.1);
        }
        self.tray_shown = Some(shown);
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
        if self.cfg.notify {
            proc::notify(state, &["-t", "1500"]);
        }
        self.update_tray();
    }

    fn toggle(&mut self) {
        let enabled = !self.engine.enabled();
        self.set_enabled(enabled);
    }

    fn reload(&mut self) -> Result<()> {
        let cfg = Config::load(&self.path)?;
        if backend::settings_changed(&self.cfg, &cfg) {
            self.engine.set_backend(backend::make(&cfg));
        }
        self.engine
            .set_selection(selection::make(&cfg, &self.selection_active));
        self.engine.set_config(cfg.clone());
        self.registry.apply(&cfg);
        self.cfg = cfg;
        self.apply_side_effects();
        self.update_tray();
        info!("конфиг перечитан");
        Ok(())
    }

    fn status(&mut self) -> Status {
        let layouts = self.engine.layout_info();
        let selection = if self.engine.selection_enabled() {
            self.selection.clone()
        } else {
            Err("выключено в настройках".into())
        };
        Status {
            version: env!("CARGO_PKG_VERSION").into(),
            pid: std::process::id(),
            enabled: self.engine.enabled(),
            layouts,
            selection: selection.is_ok(),
            selection_via: selection.clone().ok().map(Into::into),
            selection_note: selection.err(),
            tray: self.tray.as_ref().is_some_and(TrayIcon::visible),
            tray_waiting: self.tray.as_ref().is_some_and(|tray| !tray.visible()),
            devices: self.registry.snapshot(),
            actions: self.engine.actions(),
            log: logbuf::recent(),
            config_path: self.path.display().to_string(),
        }
    }

    /// Ответ на запрос из сокета.
    fn answer(&mut self, req: Request) -> Response {
        match req {
            Request::Status => Response {
                status: Some(Box::new(self.status())),
                ..Response::ok()
            },
            Request::SetEnabled { enabled } => {
                self.set_enabled(enabled);
                Response::ok()
            }
            Request::Toggle => {
                self.toggle();
                Response::ok()
            }
            Request::Reload => match self.reload() {
                Ok(()) => Response::ok(),
                Err(err) => Response::error(format!("{err:#}")),
            },
            // Выход — по Msg::Quit: сокет шлёт его, когда ответ уже
            // записан (daemon/socket.rs).
            Request::Quit => Response::ok(),
        }
    }

    /// `false` — пора выходить.
    fn handle(&mut self, msg: Msg) -> bool {
        match msg {
            Msg::Input(Event::Key { key, value, ts }) => self.engine.key(key, value, ts),
            Msg::Input(Event::Button { value, ts }) => self.engine.button(value, ts),
            Msg::Input(Event::Touch { device, value, ts }) => {
                self.engine.touch(&device, value, ts);
            }
            Msg::Input(Event::Dropped) => self.engine.resync(self.registry.held_keys()),
            Msg::Input(Event::Gone { name }) => {
                info!("устройство отключено: {name}");
                self.engine.resync(self.registry.held_keys());
            }
            Msg::Selection(selection::Event::Primary(size)) => self.engine.primary_changed(size),
            Msg::Selection(selection::Event::Watch(state)) => {
                if state != self.selection {
                    match &state {
                        Ok(via) => info!("слежу за выделением (PRIMARY) через {via}"),
                        Err(reason) => warn!("выделение не отслеживается: {reason}"),
                    }
                    self.selection = state;
                }
            }
            Msg::Keymap(keymap) => {
                self.engine.set_keymap(keymap);
                self.update_tray();
            }
            Msg::TogglePause => self.toggle(),
            Msg::Reload => {
                if let Err(err) = self.reload() {
                    error!("конфиг не перечитан: {err:#}");
                }
            }
            Msg::Quit => return false,
            Msg::Ipc {
                req,
                deadline,
                reply,
            } => {
                if Instant::now() > deadline {
                    debug!("IPC: {req:?} не дождался очереди — пропущен");
                } else {
                    let _ = reply.send(self.answer(req));
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Mutex;

    use evdev::KeyCode;

    use super::*;
    use crate::engine::{Backend, Keys, Selection};
    use crate::testutil::{NoKeys, TempDir};

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

    pub(super) fn daemon(path: PathBuf) -> Daemon {
        let cfg = Config {
            tray: false,
            selection: false,
            notify: false,
            ..Config::default()
        };
        let (tx, _rx) = mpsc::channel();
        let registry = Registry::new(&cfg, |_| true);
        let engine = Engine::new(
            cfg.clone(),
            Box::new(TwoLayouts(0)),
            Box::new(NoKeys),
            None,
            Hooks {
                caps_on: Box::new(|| false),
                clock: Box::new(monotonic),
                guard: Box::new(|| None),
            },
        );
        Daemon::new(path, cfg, engine, registry, tx, Active::default())
    }

    fn ask(d: &mut Daemon, req: Request) -> (bool, Response) {
        let (reply, answer) = mpsc::channel();
        let alive = d.handle(Msg::Ipc {
            req,
            deadline: Instant::now() + Duration::from_secs(60),
            reply,
        });
        (alive, answer.recv().unwrap())
    }

    #[test]
    fn answers_gui_requests() {
        let dir = TempDir::new("daemon");
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

        // На quit — только ответ; выход — по Msg::Quit.
        let (alive, resp) = ask(&mut d, Request::Quit);
        assert!(alive && resp.ok);
        assert!(!d.handle(Msg::Quit));
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
            assert!(d.handle(Msg::Input(Event::Key { key, value, ts })));
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
    fn reports_selection_watcher_state() {
        struct NoSelection;
        impl Selection for NoSelection {
            fn read_primary(&mut self) -> Option<String> {
                None
            }
            fn paste(&mut self, _: &str, _: &mut dyn Keys) -> Result<()> {
                Ok(())
            }
        }

        let mut d = daemon(std::env::temp_dir().join("shiftfix-none.toml"));
        d.engine.set_selection(Some(Box::new(NoSelection)));
        let status = |d: &mut Daemon| {
            let s = ask(d, Request::Status).1.status.unwrap();
            (s.selection, s.selection_note, s.selection_via)
        };

        d.handle(Msg::Selection(selection::Event::Watch(Err(
            "нет wl-paste".into()
        ))));
        assert_eq!(status(&mut d), (false, Some("нет wl-paste".into()), None));
        d.handle(Msg::Selection(selection::Event::Watch(Ok("X11"))));
        assert_eq!(status(&mut d), (true, None, Some("X11".into())));
        d.engine.set_selection(None);
        assert_eq!(
            status(&mut d),
            (false, Some("выключено в настройках".into()), None)
        );
    }

    #[test]
    fn tray_follows_layout_list() {
        struct Listed(Arc<Mutex<Vec<String>>>);
        impl Backend for Listed {
            fn name(&self) -> &'static str {
                "тест"
            }
            fn layouts(&mut self) -> Result<Vec<String>> {
                Ok(self.0.lock().unwrap().clone())
            }
            fn current(&mut self) -> Result<Option<usize>> {
                Ok(Some(0))
            }
            fn set(&mut self, _: usize, _: &mut dyn Keys) -> Result<()> {
                Ok(())
            }
            fn toggle(&mut self, _: &mut dyn Keys) -> Result<()> {
                Ok(())
            }
        }

        let mut d = daemon(std::env::temp_dir().join("shiftfix-none.toml"));
        let codes = Arc::new(Mutex::new(vec!["us".to_owned(), "ru".to_owned()]));
        d.engine.set_backend(Box::new(Listed(codes.clone())));
        // Значка в тесте нет, но служба помнит, что ему отдала.
        d.cfg.tray = true;
        d.update_tray();
        assert_eq!(d.tray_shown, Some((true, "us · ru".into())));

        // Раскладку добавили в настройках системы — пришла новая keymap.
        codes.lock().unwrap().push("de".into());
        assert!(d.handle(Msg::Keymap(Keymap::default())));
        assert_eq!(d.tray_shown, Some((true, "us · ru · de".into())));
        assert!(d.handle(Msg::TogglePause));
        assert_eq!(d.tray_shown, Some((false, "us · ru · de".into())));
    }

    #[test]
    fn second_instance_is_refused() {
        let dir = TempDir::new("lock");
        let path = dir.join("shiftfix.lock");
        let first = lock_file(&path).unwrap();
        assert!(lock_file(&path).is_err());
        drop(first);
        // Копия дескриптора с замком ещё может жить у процесса, которого как
        // раз запускает соседний тест (fork до exec), — подождать, пока она
        // закроется.
        let started = Instant::now();
        let mut second = lock_file(&path);
        while second.is_err() && started.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(10));
            second = lock_file(&path);
        }
        assert!(second.is_ok());
    }
}
