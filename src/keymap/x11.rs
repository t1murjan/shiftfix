//! Keymap сеанса X11: X-сервер не присылает её, она собирается по именам
//! раскладок из свойства `_XKB_RULES_NAMES` корневого окна.

use anyhow::Result;
use log::{debug, info};
use x11rb::connection::Connection as _;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{ChangeWindowAttributesAux, ConnectionExt as _, EventMask};

use super::{Keymap, xkb};
use crate::sys::x11::{XConn, connect, rules_names};

/// Раскладки сеанса X11 из `_XKB_RULES_NAMES` (`None` — свойство не прочитать
/// или нет libxkbcommon).
pub fn fetch(x: &XConn) -> Option<Keymap> {
    let names = rules_names(x).ok()?;
    let keymap = xkb::from_names(&names.layout, &names.variant, &names.options);
    if keymap.is_none() {
        debug!("keymap X11 не разобрана (нет libxkbcommon?)");
    }
    keymap
}

/// Присылать keymap сеанса X11 сейчас и при каждой смене раскладок, пока жив
/// X-сервер. В Wayland keymap присылает композитор (`wayland.rs`).
pub fn watch(on_change: &impl Fn(Keymap)) -> Result<()> {
    let x = connect()?;
    let conn = &x.conn;
    let rules = conn.intern_atom(false, b"_XKB_RULES_NAMES")?.reply()?.atom;
    conn.change_window_attributes(
        x.root,
        &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
    )?;
    conn.flush()?;
    let send = || {
        if let Some(keymap) = fetch(&x) {
            info!("раскладки из X11: {}", keymap.names());
            on_change(keymap);
        }
    };
    send();
    loop {
        if let Event::PropertyNotify(ev) = conn.wait_for_event()?
            && ev.atom == rules
        {
            send();
        }
    }
}
