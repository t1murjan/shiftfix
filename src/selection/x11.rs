//! X11 и Wayland без data-control (GNOME): выделение видно через X-сервер
//! или XWayland — слежение через XFixes, чтение и вставка через xclip.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;
use x11rb::connection::Connection as _;
use x11rb::protocol::Event;
use x11rb::protocol::xfixes::{ConnectionExt as _, SelectionEventMask};
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, CreateWindowAux, WindowClass};

use super::clipboard::{Clipboard, Paste, UTF8};
use crate::config::Config;
use crate::engine::{Keys, Selection};
use crate::sys::proc::{TIMEOUT, feed, run};
use crate::sys::x11::connect;

pub struct XClipboard {
    paste: Paste,
    xclip: String,
}

impl XClipboard {
    pub fn new(cfg: &Config, xclip: &str) -> Self {
        XClipboard {
            paste: Paste::new(cfg),
            xclip: xclip.into(),
        }
    }

    fn read_from(&self, selection: &str, target: &str, timeout: Duration) -> Option<Vec<u8>> {
        let out = run(
            &self.xclip,
            &["-o", "-selection", selection, "-t", target],
            timeout,
        )
        .ok()?;
        out.ok.then_some(out.stdout)
    }
}

impl Clipboard for XClipboard {
    const TEXT: &str = "UTF8_STRING";

    fn types(&self) -> Option<Vec<String>> {
        let data = self.read_from("clipboard", "TARGETS", Duration::from_secs(1))?;
        Some(
            String::from_utf8_lossy(&data)
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect(),
        )
    }

    fn read(&self, target: &str) -> Option<Vec<u8>> {
        self.read_from("clipboard", target, TIMEOUT)
    }

    fn copy(&self, data: &[u8], target: &str) -> Result<()> {
        // xclip уходит в фон и отдаёт данные, пока буфер не займёт кто-то другой.
        feed(
            &self.xclip,
            &["-i", "-selection", "clipboard", "-t", target],
            data,
        )
    }

    /// Очистить буфер X11 нельзя без владельца — кладём пустую строку.
    fn clear(&self) -> Result<()> {
        self.copy(b"", Self::TEXT)
    }
}

impl Selection for XClipboard {
    fn read_primary(&mut self) -> Option<String> {
        ["UTF8_STRING", UTF8, "STRING"]
            .iter()
            .find_map(|target| self.read_from("primary", target, TIMEOUT))
            .map(|data| String::from_utf8_lossy(&data).into_owned())
    }

    fn paste(&mut self, text: &str, keys: &mut dyn Keys) -> Result<()> {
        self.paste.run(self, text, keys)
    }
}

/// Следить за PRIMARY, пока жив X-сервер. `Ok(())` — служба завершается
/// или слежение остановлено (`stop`).
pub fn watch(send: &dyn Fn(super::Event) -> bool, stop: &AtomicBool) -> Result<()> {
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
    if !send(super::Event::Watch(Ok("X11"))) {
        return Ok(());
    }
    loop {
        let event = conn.wait_for_event()?;
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        if let Event::XfixesSelectionNotify(ev) = event
            && ev.selection == primary
            && !send(super::Event::Primary(usize::from(ev.owner != x11rb::NONE)))
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
        let mut app = App::new(&dir);
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
