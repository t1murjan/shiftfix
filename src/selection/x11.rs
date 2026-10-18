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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::super::clipboard::testing::App;
    use super::*;
    use crate::testutil::{TempDir, write_script};

    /// Фальшивый xclip: выделения — файлы `clip`/`primary` и их цели.
    fn clipboard(dir: &Path) -> XClipboard {
        let xclip = r#"d="$(dirname "$0")"
[ "$2" = "-selection" ] && [ "$4" = "-t" ] || exit 2
sel=$3; [ "$sel" = clipboard ] && sel=clip
if [ "$1" = "-i" ]; then cat > "$d/$sel"; printf '%s\n' "$5" > "$d/$sel.types"; exit 0; fi
[ -f "$d/$sel" ] || { echo "Error: target $5 not available" >&2; exit 1; }
if [ "$5" = TARGETS ]; then printf 'TARGETS\n'; cat "$d/$sel.types"; exit 0; fi
grep -qx "$5" "$d/$sel.types" || { echo "Error: target $5 not available" >&2; exit 1; }
cat "$d/$sel""#;
        let cfg = Config {
            paste_restore: Duration::ZERO,
            ..Config::default()
        };
        XClipboard::new(&cfg, &write_script(dir, "xclip", xclip))
    }

    #[test]
    fn reads_primary() {
        let dir = TempDir::new("xclip-primary");
        let mut clip = clipboard(&dir);
        assert_eq!(clip.read_primary(), None);
        fs::write(dir.join("primary"), "Ghbdtn").unwrap();
        fs::write(dir.join("primary.types"), "STRING\n").unwrap();
        assert_eq!(clip.read_primary().as_deref(), Some("Ghbdtn"));
    }

    #[test]
    fn pastes_and_restores_previous_clipboard() {
        let dir = TempDir::new("xclip-paste");
        let mut clip = clipboard(&dir);
        fs::write(dir.join("clip"), [0x89, b'P', b'N', b'G']).unwrap();
        fs::write(dir.join("clip.types"), "image/png\n").unwrap();
        let mut app = App {
            dir: dir.to_path_buf(),
            pasted: Vec::new(),
        };
        clip.paste("Привет мир", &mut app).unwrap();
        assert_eq!(app.pasted, ["Привет мир"]);
        assert_eq!(
            fs::read(dir.join("clip")).unwrap(),
            [0x89, b'P', b'N', b'G']
        );
        assert_eq!(
            fs::read_to_string(dir.join("clip.types")).unwrap(),
            "image/png\n"
        );

        // Пустой буфер обмена очистить в X11 нельзя — после вставки в нём пустая строка.
        fs::remove_file(dir.join("clip")).unwrap();
        clip.paste("ещё", &mut app).unwrap();
        assert_eq!(app.pasted.last().unwrap(), "ещё");
        assert_eq!(fs::read_to_string(dir.join("clip")).unwrap(), "");

        // Пароль из менеджера паролей обратно не кладётся.
        fs::write(dir.join("clip"), "s3cret").unwrap();
        fs::write(
            dir.join("clip.types"),
            "UTF8_STRING\nx-kde-passwordManagerHint\n",
        )
        .unwrap();
        clip.paste("и ещё", &mut app).unwrap();
        assert_eq!(app.pasted.last().unwrap(), "и ещё");
        assert_eq!(fs::read_to_string(dir.join("clip")).unwrap(), "");
    }
}
