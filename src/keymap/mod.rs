//! Таблицы раскладок сеанса: какой символ даёт каждая клавиша.
//!
//! Таблицы строятся из настоящей keymap: её присылает композитор Wayland
//! (`wayland.rs`) или собирает X-сервер (`x11.rs`), а разбирает системная
//! libxkbcommon (`xkb.rs`). Без неё остаются встроенные таблицы us/ru
//! (`layout.rs`). По таблицам перекодируется выделенный текст (`convert.rs`).

mod convert;
mod layout;
pub mod wayland;
pub mod x11;
mod xkb;

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use log::debug;

pub use convert::{Tables, convert_text, detect_source, next_with_table, plan_strokes};
pub use layout::{Layout, builtin, code_from_long_name, norm_layout};
pub use xkb::{from_names, from_text};

use crate::sys::env;
use crate::sys::wayland::connect;

/// Раскладки сессии: таблица и человеческое имя каждой группы.
#[derive(Clone, Debug, Default)]
pub struct Keymap {
    pub layouts: Vec<Arc<Layout>>,
    /// CapsLock переключает раскладку, а не регистр (grp:caps_toggle и подобные).
    pub caps_switches_group: bool,
}

impl Keymap {
    /// Имена раскладок через запятую — для журнала и `--check`.
    pub fn names(&self) -> String {
        let names: Vec<&str> = self.layouts.iter().map(|l| l.name.as_str()).collect();
        names.join(", ")
    }
}

/// Следить за keymap: при смене списка раскладок композитор пришлёт новую.
pub fn spawn_watcher(on_change: impl Fn(Keymap) + Send + 'static) {
    let spawned = thread::Builder::new().name("keymap".into()).spawn(move || {
        loop {
            if let Some(conn) = connect() {
                wayland::watch(&conn, &on_change);
            } else if env::wayland_display().is_none() && env::x11_display().is_some() {
                // Сеанс X11: keymap строится из свойства корневого окна — и тогда,
                // когда выделение не исправляется.
                if let Err(err) = x11::watch(&on_change) {
                    debug!("keymap X11: {err:#}");
                }
            }
            thread::sleep(Duration::from_secs(10)); // сессии ещё нет или она перезапущена
        }
    });
    if let Err(err) = spawned {
        log::warn!("поток keymap не запустился: {err}");
    }
}
