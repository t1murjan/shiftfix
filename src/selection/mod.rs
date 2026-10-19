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
pub(crate) mod wayland;
pub mod x11;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::config::Config;
use crate::engine::{Keys, Selection};
use crate::sys::wayland as compositor;
use crate::sys::{env, paths};
use wayland::WlClipboard;
use x11::XClipboard;

/// Событие слежения за выделением для службы.
pub enum Event {
    /// Слежение: `Ok(чем)` — работает, иначе причина, почему нет.
    Watch(Result<&'static str, String>),
    /// PRIMARY изменился; размер нового выделения в байтах. X11 размера не
    /// знает и шлёт 1 (непустое) или 0. Движку важно только, пустое ли.
    Primary(usize),
}

/// Источник выделения.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Source {
    None,
    Wayland,
    X11,
}

/// Какой источник выделения сейчас работает (его выставляет поток слежения).
#[derive(Clone, Default)]
pub(crate) struct Active(Arc<AtomicU8>);

impl Active {
    fn set(&self, source: Source) {
        self.0.store(source as u8, Ordering::Relaxed);
    }

    fn get(&self) -> Source {
        match self.0.load(Ordering::Relaxed) {
            1 => Source::Wayland,
            2 => Source::X11,
            _ => Source::None,
        }
    }
}

/// Чтение и вставка через тот источник, за которым сейчас следим.
pub(crate) struct AutoSelection {
    active: Active,
    wayland: WlClipboard,
    x11: XClipboard,
}

impl Selection for AutoSelection {
    fn read_primary(&mut self) -> Option<String> {
        match self.active.get() {
            Source::Wayland => self.wayland.read_primary(),
            Source::X11 => self.x11.read_primary(),
            Source::None => None,
        }
    }

    fn paste(&mut self, text: &str, keys: &mut dyn Keys) -> Result<()> {
        match self.active.get() {
            Source::Wayland => self.wayland.paste(text, keys),
            Source::X11 => self.x11.paste(text, keys),
            Source::None => bail!("выделение сейчас недоступно"),
        }
    }
}

/// `None`, если исправление выделения выключено в настройках.
pub(crate) fn make(cfg: &Config, active: &Active) -> Option<Box<dyn Selection>> {
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

/// Что известно о сеансе для выбора.
struct Facts {
    wayland: bool,
    /// Есть ли у композитора Wayland data-control; `None` — он не отвечает.
    data_control: Option<bool>,
    wl_clipboard: bool,
    x11: bool,
    xclip: bool,
}

impl Facts {
    fn gather() -> Facts {
        let wayland = env::wayland_display().is_some();
        Facts {
            wayland,
            data_control: if wayland {
                compositor::data_control()
            } else {
                None
            },
            wl_clipboard: paths::in_path("wl-paste") && paths::in_path("wl-copy"),
            x11: env::x11_display().is_some(),
            xclip: paths::in_path("xclip"),
        }
    }
}

pub fn decide() -> Choice {
    choose(&Facts::gather())
}

fn choose(f: &Facts) -> Choice {
    let (x11, xclip) = (f.x11, f.xclip);
    if f.wayland {
        match f.data_control {
            Some(true) if f.wl_clipboard => return Choice::Wayland,
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

/// Поток слежения за выделением. Когда значение выброшено (исправление
/// выделения выключили), поток останавливается — при следующем изменении
/// выделения или следующей проверке сеанса.
pub(crate) struct Watcher {
    stop: Arc<AtomicBool>,
    active: Active,
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.active.set(Source::None);
    }
}

/// `send` — куда отдавать события; `false` от неё — получателя больше нет.
pub(crate) fn spawn_watcher(
    send: impl Fn(Event) -> bool + Send + 'static,
    active: Active,
) -> Option<Watcher> {
    let stop = Arc::new(AtomicBool::new(false));
    let watcher = Watcher {
        stop: Arc::clone(&stop),
        active: active.clone(),
    };
    let spawned = thread::Builder::new()
        .name("selection".into())
        .spawn(move || run(&send, &active, &stop));
    match spawned {
        Ok(_) => Some(watcher),
        Err(err) => {
            log::warn!("поток слежения за выделением не запустился: {err}");
            None
        }
    }
}

fn run(send: &dyn Fn(Event) -> bool, active: &Active, stop: &AtomicBool) {
    let stopped = || stop.load(Ordering::Relaxed);
    let mut failures = 0u32;
    while !stopped() {
        let started = Instant::now();
        let watched = match decide() {
            Choice::Wait(reason, secs) => {
                if stopped() || !send(Event::Watch(Err(reason))) {
                    return;
                }
                thread::sleep(Duration::from_secs(secs));
                continue;
            }
            Choice::Wayland => {
                active.set(Source::Wayland);
                wayland::watch(send, stop, "wl-paste")
            }
            Choice::X11 => {
                active.set(Source::X11);
                x11::watch(send, stop).context("X11")
            }
        };
        if stopped() {
            return; // источник уже сброшен, а новый поток мог выставить свой
        }
        active.set(Source::None);
        let Err(err) = watched else {
            return; // служба завершается
        };
        if !send(Event::Watch(Err(format!("{err:#}")))) {
            return;
        }
        // Слежение работало и оборвалось (перезапуск композитора) — пробуем снова,
        // при повторных сбоях всё реже.
        if started.elapsed() > Duration::from_secs(30) {
            failures = 0;
        }
        failures += 1;
        let secs = 2u64.saturating_pow(failures.min(6)).min(60);
        thread::sleep(Duration::from_secs(secs));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chooses_source() {
        let kde = Facts {
            wayland: true,
            data_control: Some(true),
            wl_clipboard: true,
            x11: true,
            xclip: true,
        };
        let pick = |change: &dyn Fn(&mut Facts)| {
            let mut f = Facts { ..kde };
            change(&mut f);
            match choose(&f) {
                Choice::Wait(_, secs) => format!("ждать {secs}"),
                choice => format!("{choice:?}"),
            }
        };
        assert_eq!(pick(&|_| {}), "Wayland");
        assert_eq!(pick(&|f| f.wl_clipboard = false), "X11");
        assert_eq!(
            pick(&|f| {
                f.wl_clipboard = false;
                f.xclip = false;
            }),
            "ждать 30"
        );
        assert_eq!(pick(&|f| f.data_control = None), "ждать 5");
        // GNOME: без data-control — через XWayland, если есть xclip.
        assert_eq!(pick(&|f| f.data_control = Some(false)), "X11");
        assert_eq!(
            pick(&|f| {
                f.data_control = Some(false);
                f.xclip = false;
            }),
            "ждать 30"
        );
        assert_eq!(
            pick(&|f| {
                f.data_control = Some(false);
                f.x11 = false;
            }),
            "ждать 30"
        );
        // X11 без Wayland.
        let x11 = |f: &mut Facts| {
            f.wayland = false;
            f.data_control = None;
        };
        assert_eq!(pick(&x11), "X11");
        assert_eq!(
            pick(&|f| {
                x11(f);
                f.xclip = false;
            }),
            "ждать 30"
        );
        assert_eq!(
            pick(&|f| {
                x11(f);
                f.x11 = false;
            }),
            "ждать 5"
        );
    }
}
