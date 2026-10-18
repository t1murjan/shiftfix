//! Связь со службой: опрос состояния в фоне, команды, управление systemd.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use eframe::egui;
use shiftfix::config::Config;
use shiftfix::devices::{self, DeviceInfo};
use shiftfix::ipc::{self, Request, Status};
use shiftfix::service;
use shiftfix::sys::env;
use shiftfix::sys::proc;

use crate::toast::{Toast, ToastKind};

use shiftfix::service::UNIT;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Service {
    /// Есть пользовательский systemd.
    pub systemd: bool,
    /// Юнит установлен (или файл автозапуска без systemd).
    pub installed: bool,
    pub autostart: bool,
}

#[derive(Clone, Default)]
pub struct Snapshot {
    /// Ответ службы; окно берёт снимок каждый кадр, поэтому без копии.
    pub status: Option<Arc<Status>>,
    pub service: Service,
    /// Последние строки журнала службы, когда она не отвечает.
    pub journal: Vec<String>,
    /// Устройства, которые окно нашло само, пока служба не отвечает
    /// (`None` — ещё не искало).
    pub local_devices: Option<Vec<DeviceInfo>>,
    pub updated: Option<Instant>,
}

#[derive(Clone, Copy, Debug)]
pub enum ServiceAction {
    Start,
    Stop,
    Restart,
    Autostart(bool),
    Install,
}

pub struct Link {
    shared: Arc<Mutex<Snapshot>>,
    toasts: Sender<Toast>,
    /// Разбудить поток опроса раньше срока; `true` — заодно спросить systemd.
    wake: Sender<bool>,
    /// Открыта страница «Устройства».
    devices_shown: Arc<AtomicBool>,
}

impl Link {
    /// `config` — файл окна: по нему роли устройств, найденных без службы.
    pub fn start(ctx: &egui::Context, toasts: Sender<Toast>, config: PathBuf) -> Link {
        let shared = Arc::new(Mutex::new(Snapshot::default()));
        let (wake, woken) = mpsc::channel();
        let devices_shown = Arc::new(AtomicBool::new(false));
        let link = Link {
            shared: shared.clone(),
            toasts,
            wake,
            devices_shown: devices_shown.clone(),
        };
        let ctx = ctx.clone();
        // Снимок пишет только этот поток: иначе ответ, полученный до команды,
        // мог лечь поверх полученного после, и переключатель откатывался.
        thread::Builder::new()
            .name("link".into())
            .spawn(move || {
                let mut round = 0u32;
                let mut with_service = true;
                loop {
                    // Без службы устройства ищем сами, но только для открытой
                    // страницы: незачем открывать все /dev/input/event* каждые 2 с.
                    let scan = with_service && devices_shown.load(Ordering::Relaxed);
                    refresh(&shared, with_service, scan.then_some(config.as_path()));
                    ctx.request_repaint();
                    round = round.wrapping_add(1);
                    // Раз в 400 мс, systemd — каждый пятый раз; команда будит раньше.
                    with_service = match woken.recv_timeout(Duration::from_millis(400)) {
                        Ok(service) => woken.try_iter().fold(service, |a, s| a || s),
                        Err(RecvTimeoutError::Timeout) => round.is_multiple_of(5),
                        Err(RecvTimeoutError::Disconnected) => return,
                    };
                }
            })
            .expect("поток связи со службой");
        link
    }

    pub fn snapshot(&self) -> Snapshot {
        self.shared.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Спросить службу и systemd сейчас, не дожидаясь очередного опроса.
    pub fn poll_now(&self) {
        let _ = self.wake.send(true);
    }

    /// Открыта ли страница «Устройства». Только что открыли — искать сразу.
    pub fn show_devices(&self, shown: bool) {
        let was = self.devices_shown.swap(shown, Ordering::Relaxed);
        if shown && !was {
            self.poll_now();
        }
    }

    /// Команда службе в фоне; по ошибке — всплывающее сообщение.
    pub fn send(&self, req: Request, success: Option<&'static str>) {
        let (toasts, wake) = (self.toasts.clone(), self.wake.clone());
        thread::spawn(move || {
            let toast = match ipc::request(&req) {
                Ok(_) => success.map(|text| Toast::new(ToastKind::Success, text)),
                Err(err) => Some(Toast::new(ToastKind::Error, format!("{err:#}"))),
            };
            if let Some(toast) = toast {
                let _ = toasts.send(toast);
            }
            let _ = wake.send(false);
        });
    }

    pub fn service(&self, action: ServiceAction) {
        let (toasts, wake) = (self.toasts.clone(), self.wake.clone());
        thread::spawn(move || {
            let toast = match run_service_action(action) {
                Ok(text) => Toast::new(ToastKind::Success, text),
                Err(err) => Toast::new(ToastKind::Error, format!("{err:#}")),
            };
            let _ = toasts.send(toast);
            thread::sleep(Duration::from_millis(600)); // дать службе подняться
            let _ = wake.send(true);
        });
    }
}

/// Обновить снимок. `devices_config` — заодно поискать устройства, если
/// служба не отвечает.
fn refresh(shared: &Arc<Mutex<Snapshot>>, with_service: bool, devices_config: Option<&Path>) {
    let result = ipc::request(&Request::Status);
    let service = with_service.then(query_service);
    // journalctl и поиск устройств идут долго: под замком они держали бы кадр окна.
    let journal = (result.is_err() && with_service).then(journal);
    let local_devices = devices_config
        .filter(|_| result.is_err())
        .map(local_devices);
    let Ok(mut snap) = shared.lock() else { return };
    match result {
        Ok(response) => {
            snap.status = response.status.map(Arc::from);
            snap.journal.clear();
            snap.local_devices = None;
        }
        Err(_) => {
            snap.status = None;
            if let Some(journal) = journal {
                snap.journal = journal;
            }
            if local_devices.is_some() {
                snap.local_devices = local_devices;
            }
        }
    }
    if let Some(service) = service {
        snap.service = service;
    }
    snap.updated = Some(Instant::now());
}

fn systemctl(args: &[&str]) -> Result<proc::Output> {
    let mut full = vec!["--user"];
    full.extend_from_slice(args);
    proc::run("systemctl", &full, Duration::from_secs(10))
}

fn query_service() -> Service {
    if env::in_path("systemctl")
        && let Ok(out) = systemctl(&["show", UNIT, "--property=LoadState,UnitFileState"])
        && out.ok
    {
        let text = out.text();
        let value = |key: &str| {
            text.lines()
                .find_map(|l| l.strip_prefix(key).and_then(|v| v.strip_prefix('=')))
                .unwrap_or_default()
                .to_owned()
        };
        return Service {
            systemd: true,
            installed: value("LoadState") == "loaded",
            autostart: value("UnitFileState") == "enabled",
        };
    }
    let autostart = autostart_file().exists();
    Service {
        systemd: false,
        installed: autostart,
        autostart,
    }
}

fn journal() -> Vec<String> {
    let Ok(out) = proc::run(
        "journalctl",
        &["--user", "-u", UNIT, "-n", "60", "--no-pager", "-o", "cat"],
        Duration::from_secs(4),
    ) else {
        return Vec::new();
    };
    out.text().lines().map(str::to_owned).collect()
}

/// Устройства так, как их увидит служба с конфигом `config`.
fn local_devices(config: &Path) -> Vec<DeviceInfo> {
    let cfg = Config::load(config).unwrap_or_default();
    devices::event_nodes()
        .iter()
        .filter_map(|p| devices::describe(p, &cfg))
        .collect()
}

fn autostart_file() -> PathBuf {
    env::config_home()
        .join("autostart")
        .join("shiftfix.desktop")
}

fn daemon_binary() -> PathBuf {
    let path = env::sibling_binary("shiftfix");
    fs::canonicalize(&path).unwrap_or(path)
}

fn unit_text() -> String {
    service::unit_text(&daemon_binary(), service::graphical_target_active())
}

fn checked(out: proc::Output, what: &str) -> Result<()> {
    if !out.ok {
        bail!(
            "{what}: {}",
            if out.stderr.is_empty() {
                "ошибка"
            } else {
                &out.stderr
            }
        );
    }
    Ok(())
}

fn run_service_action(action: ServiceAction) -> Result<String> {
    let systemd = env::in_path("systemctl") && systemctl(&["show-environment"]).is_ok_and(|o| o.ok);
    if !systemd {
        return run_without_systemd(action);
    }
    match action {
        ServiceAction::Start => {
            checked(systemctl(&["start", UNIT])?, "запуск").map(|()| "Служба запущена".into())
        }
        ServiceAction::Stop => {
            checked(systemctl(&["stop", UNIT])?, "остановка").map(|()| "Служба остановлена".into())
        }
        ServiceAction::Restart => checked(systemctl(&["restart", UNIT])?, "перезапуск")
            .map(|()| "Служба перезапущена".into()),
        ServiceAction::Autostart(true) => checked(systemctl(&["enable", UNIT])?, "автозапуск")
            .map(|()| "Автозапуск включён".into()),
        ServiceAction::Autostart(false) => checked(systemctl(&["disable", UNIT])?, "автозапуск")
            .map(|()| "Автозапуск выключен".into()),
        ServiceAction::Install => {
            let path = env::config_home().join("systemd/user").join(UNIT);
            fs::create_dir_all(path.parent().expect("parent")).context("каталог юнитов systemd")?;
            fs::write(&path, unit_text())
                .with_context(|| format!("не записывается {}", path.display()))?;
            checked(systemctl(&["daemon-reload"])?, "daemon-reload")?;
            checked(systemctl(&["enable", "--now", UNIT])?, "включение службы")?;
            Ok("Служба установлена и запущена".into())
        }
    }
}

fn run_without_systemd(action: ServiceAction) -> Result<String> {
    match action {
        ServiceAction::Start | ServiceAction::Restart => {
            if matches!(action, ServiceAction::Restart) {
                let _ = ipc::request(&Request::Quit);
                thread::sleep(Duration::from_millis(500));
            }
            proc::spawn_detached(daemon_binary(), &[])?;
            Ok("Служба запущена".into())
        }
        ServiceAction::Stop => ipc::request(&Request::Quit).map(|_| "Служба остановлена".into()),
        ServiceAction::Autostart(true) | ServiceAction::Install => {
            let path = autostart_file();
            fs::create_dir_all(path.parent().expect("parent"))?;
            // Тот же текст, что пишет install.sh (`shiftfix --print-autostart`).
            fs::write(&path, service::autostart_text(&daemon_binary()))?;
            if matches!(action, ServiceAction::Install) {
                proc::spawn_detached(daemon_binary(), &[])?;
            }
            Ok("Автозапуск включён".into())
        }
        ServiceAction::Autostart(false) => {
            let _ = fs::remove_file(autostart_file());
            Ok("Автозапуск выключен".into())
        }
    }
}

/// `shiftfix --check` в фоне.
pub fn run_check(out: Arc<Mutex<Option<String>>>, ctx: egui::Context) {
    thread::spawn(move || {
        let text = match proc::run(
            &daemon_binary().to_string_lossy(),
            &["--check"],
            Duration::from_secs(20),
        ) {
            Ok(o) => {
                let mut text = o.text();
                if !o.stderr.is_empty() {
                    text.push_str(&o.stderr);
                }
                text
            }
            Err(err) => format!("Не удалось запустить проверку: {err:#}"),
        };
        if let Ok(mut slot) = out.lock() {
            *slot = Some(text);
        }
        ctx.request_repaint();
    });
}
