//! Откуда брать выделение (PRIMARY):
//!
//! - Wayland с протоколом data-control (KDE, Hyprland, Sway…) — wl-clipboard;
//! - X11 и Wayland без data-control (GNOME) — через X-сервер или XWayland, xclip.
//!
//! Здесь — выбор источника и поток слежения; чтение и вставка — в `wayland.rs`
//! и `x11.rs`, общее для вставки через буфер обмена — в `clipboard.rs`.
//!
//! Выбор делается на ходу и пересматривается, если сеанс сменился: служба
//! может стартовать раньше сеанса или пережить его перезапуск.

mod clipboard;
pub mod wayland;
pub mod x11;

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};

use crate::config::Config;
use crate::daemon::Msg;
use crate::engine::{Keys, Selection};
use crate::sys::env;
use crate::sys::wayland as compositor;
use wayland::WlClipboard;
use x11::XClipboard;

const NONE: u8 = 0;
const WAYLAND: u8 = 1;
const X11: u8 = 2;

/// Какой источник выделения сейчас работает (его выставляет поток слежения).
#[derive(Clone, Default)]
pub struct Active(Arc<AtomicU8>);

impl Active {
    fn set(&self, value: u8) {
        self.0.store(value, Ordering::Relaxed);
    }

    fn get(&self) -> u8 {
        self.0.load(Ordering::Relaxed)
    }
}

/// Чтение и вставка через тот источник, за которым сейчас следим.
pub struct AutoSelection {
    active: Active,
    wayland: WlClipboard,
    x11: XClipboard,
}

impl Selection for AutoSelection {
    fn read_primary(&mut self) -> Option<String> {
        match self.active.get() {
            WAYLAND => self.wayland.read_primary(),
            X11 => self.x11.read_primary(),
            _ => None,
        }
    }

    fn paste(&mut self, text: &str, keys: &mut dyn Keys) -> Result<()> {
        match self.active.get() {
            WAYLAND => self.wayland.paste(text, keys),
            X11 => self.x11.paste(text, keys),
            _ => bail!("выделение сейчас недоступно"),
        }
    }
}

/// `None`, если исправление выделения выключено в настройках.
pub fn make(cfg: &Config, active: &Active) -> Option<Box<dyn Selection>> {
    cfg.selection.then(|| {
        Box::new(AutoSelection {
            active: active.clone(),
            wayland: WlClipboard::new(cfg, "wl-paste", "wl-copy"),
            x11: XClipboard::new(cfg, "xclip"),
        }) as Box<dyn Selection>
    })
}

/// Чем следить за выделением в этом сеансе.
#[derive(Debug, PartialEq, Eq)]
pub enum Choice {
    Wayland,
    X11,
    /// Пока нельзя: причина и через сколько секунд проверить снова.
    Wait(String, u64),
}

pub fn decide() -> Choice {
    let x11 = env::x11_display().is_some();
    let xclip = env::in_path("xclip");
    if env::wayland_display().is_some() {
        let wl_clipboard = env::in_path("wl-paste") && env::in_path("wl-copy");
        match compositor::data_control() {
            Some(true) if wl_clipboard => return Choice::Wayland,
            Some(true) if x11 && xclip => return Choice::X11,
            Some(true) => return Choice::Wait("не установлен wl-clipboard".into(), 30),
            None => return Choice::Wait("нет связи с композитором Wayland".into(), 5),
            Some(false) => {}
        }
        // GNOME и другие композиторы без data-control: выделение видно через XWayland.
        return match (x11, xclip) {
            (true, true) => Choice::X11,
            (true, false) => Choice::Wait(
                "композитор не даёт следить за выделением — установите xclip, \
                 тогда оно будет браться через XWayland"
                    .into(),
                30,
            ),
            (false, _) => Choice::Wait(
                "композитор не даёт следить за выделением (нет data-control), а XWayland нет"
                    .into(),
                30,
            ),
        };
    }
    match (x11, xclip) {
        (true, true) => Choice::X11,
        (true, false) => Choice::Wait("не установлен xclip".into(), 30),
        (false, _) => Choice::Wait("нет графического сеанса".into(), 5),
    }
}

pub fn spawn_watcher(tx: Sender<Msg>, active: Active) {
    let spawned = thread::Builder::new()
        .name("selection".into())
        .spawn(move || run(&tx, &active));
    if let Err(err) = spawned {
        log::warn!("поток слежения за выделением не запустился: {err}");
    }
}

fn run(tx: &Sender<Msg>, active: &Active) {
    let mut failures = 0u32;
    loop {
        let started = Instant::now();
        let (reason, wait) = match decide() {
            Choice::Wait(reason, secs) => (reason, Some(secs)),
            Choice::Wayland => {
                active.set(WAYLAND);
                let result = wayland::watch(tx);
                active.set(NONE);
                match result {
                    Some(reason) => (reason, None),
                    None => return, // служба завершается
                }
            }
            Choice::X11 => {
                active.set(X11);
                let result = x11::watch(tx);
                active.set(NONE);
                match result {
                    Ok(()) => return,
                    Err(err) => (format!("X11: {err:#}"), None),
                }
            }
        };
        if tx.send(Msg::Selection(Err(reason))).is_err() {
            return;
        }
        let secs = wait.unwrap_or_else(|| {
            // Слежение работало и оборвалось (перезапуск композитора) — пробуем снова,
            // при повторных сбоях всё реже.
            if started.elapsed() > Duration::from_secs(30) {
                failures = 0;
            }
            failures += 1;
            2u64.saturating_pow(failures.min(6)).min(60)
        });
        thread::sleep(Duration::from_secs(secs));
    }
}
