//! X11 и Wayland без data-control (GNOME): выделение видно через X-сервер
//! или XWayland — слежение через XFixes, чтение и вставка через xclip.

use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use anyhow::Result;
use evdev::KeyCode;
use log::warn;
use x11rb::connection::Connection as _;
use x11rb::protocol::Event;
use x11rb::protocol::xfixes::{ConnectionExt as _, SelectionEventMask};
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, CreateWindowAux, WindowClass};

use super::clipboard::{Restore, UTF8, is_gnome, restore_plan, wait_until};
use crate::config::Config;
use crate::daemon::Msg;
use crate::engine::{Keys, Selection};
use crate::sys::proc::{TIMEOUT, feed, run};
use crate::sys::x11::connect;

pub struct XClipboard {
    paste_keys: Vec<KeyCode>,
    restore: bool,
    restore_delay: Duration,
    xclip: String,
}

impl XClipboard {
    pub fn new(cfg: &Config, xclip: &str) -> Self {
        XClipboard {
            paste_keys: cfg.paste_keys.clone(),
            restore: cfg.restore_clipboard,
            restore_delay: cfg.paste_restore,
            xclip: xclip.into(),
        }
    }

    fn read(&self, selection: &str, target: &str, timeout: Duration) -> Option<Vec<u8>> {
        let out = run(
            &self.xclip,
            &["-o", "-selection", selection, "-t", target],
            timeout,
        )
        .ok()?;
        out.ok.then_some(out.stdout)
    }

    fn targets(&self) -> Option<Vec<String>> {
        let data = self.read("clipboard", "TARGETS", Duration::from_secs(1))?;
        Some(
            String::from_utf8_lossy(&data)
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect(),
        )
    }

    fn copy(&self, data: &[u8], target: &str) -> Result<()> {
        // xclip уходит в фон и отдаёт данные, пока буфер не займёт кто-то другой.
        feed(
            &self.xclip,
            &["-i", "-selection", "clipboard", "-t", target],
            data,
        )
    }
}

impl Selection for XClipboard {
    fn read_primary(&mut self) -> Option<String> {
        ["UTF8_STRING", UTF8, "STRING"]
            .iter()
            .find_map(|target| self.read("primary", target, TIMEOUT))
            .map(|data| String::from_utf8_lossy(&data).into_owned())
    }

    fn paste(&mut self, text: &str, keys: &mut dyn Keys) -> Result<()> {
        let saved = if self.restore {
            let plan = match self.targets() {
                None => Restore::Clear,
                Some(targets) => restore_plan(&targets, is_gnome()),
            };
            Some(match plan {
                Restore::Type(target) => match self.read("clipboard", &target, TIMEOUT) {
                    Some(data) => (Restore::Type(target), data),
                    None => (Restore::Clear, Vec::new()),
                },
                Restore::Clear => (Restore::Clear, Vec::new()),
            })
        } else {
            None
        };
        self.copy(text.as_bytes(), "UTF8_STRING")?;
        let read = || self.read("clipboard", "UTF8_STRING", Duration::from_secs(1));
        if !wait_until(read, text.as_bytes()) {
            warn!("буфер обмена не обновился вовремя, вставляю всё равно");
        }
        keys.combo(&self.paste_keys)?;
        thread::sleep(self.restore_delay);
        match saved {
            Some((Restore::Type(target), data)) => self.copy(&data, &target)?,
            // Очистить буфер X11 нельзя без владельца — кладём пустую строку.
            Some((Restore::Clear, _)) => self.copy(b"", "UTF8_STRING")?,
            None => {}
        }
        Ok(())
    }
}

/// Следить за PRIMARY, пока жив X-сервер. `Ok(())` — служба завершается.
pub fn watch(tx: &Sender<Msg>) -> Result<()> {
    let x = connect()?;
    let conn = &x.conn;
    conn.xfixes_query_version(5, 0)?.reply()?;
    let window = conn.generate_id()?;
    conn.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        window,
        x.root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_ONLY,
        x11rb::COPY_FROM_PARENT,
        &CreateWindowAux::new().override_redirect(1),
    )?;
    let primary: u32 = AtomEnum::PRIMARY.into();
    conn.xfixes_select_selection_input(
        window,
        primary,
        SelectionEventMask::SET_SELECTION_OWNER
            | SelectionEventMask::SELECTION_WINDOW_DESTROY
            | SelectionEventMask::SELECTION_CLIENT_CLOSE,
    )?;
    conn.flush()?;
    if tx.send(Msg::Selection(Ok("X11"))).is_err() {
        return Ok(());
    }
    loop {
        if let Event::XfixesSelectionNotify(ev) = conn.wait_for_event()?
            && ev.selection == primary
            && tx
                .send(Msg::Primary(usize::from(ev.owner != x11rb::NONE)))
                .is_err()
        {
            return Ok(());
        }
    }
}
