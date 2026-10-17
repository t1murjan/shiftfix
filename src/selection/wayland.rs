//! Wayland с протоколом data-control (KDE, Hyprland, Sway…): выделение
//! читается и вставляется через wl-clipboard.

use std::io::{BufRead, BufReader, Read};
use std::os::unix::process::CommandExt;
use std::process::Stdio;
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use evdev::KeyCode;
use log::{debug, warn};

use super::clipboard::{Restore, TEXT_TYPES, UTF8, is_gnome, restore_plan, wait_until};
use crate::config::Config;
use crate::daemon::Msg;
use crate::engine::{Keys, Selection};
use crate::sys::env;
use crate::sys::proc::{TIMEOUT, feed, run};

pub struct WlClipboard {
    paste_keys: Vec<KeyCode>,
    restore: bool,
    restore_delay: Duration,
    wl_paste: String,
    wl_copy: String,
}

impl WlClipboard {
    pub fn new(cfg: &Config, wl_paste: &str, wl_copy: &str) -> Self {
        WlClipboard {
            paste_keys: cfg.paste_keys.clone(),
            restore: cfg.restore_clipboard,
            restore_delay: cfg.paste_restore,
            wl_paste: wl_paste.into(),
            wl_copy: wl_copy.into(),
        }
    }

    fn types(&self, primary: bool) -> Option<Vec<String>> {
        let base: &[&str] = if primary { &["--primary"] } else { &[] };
        let list = run(
            &self.wl_paste,
            &[base, &["--list-types"][..]].concat(),
            Duration::from_secs(1),
        )
        .ok()?;
        // Ошибка — «Nothing is copied».
        list.ok.then(|| {
            String::from_utf8_lossy(&list.stdout)
                .split_whitespace()
                .map(String::from)
                .collect()
        })
    }

    fn read(&self, primary: bool, mime: &str) -> Option<Vec<u8>> {
        let base: &[&str] = if primary { &["--primary"] } else { &[] };
        let out = run(
            &self.wl_paste,
            &[base, &["--no-newline", "--type", mime][..]].concat(),
            TIMEOUT,
        )
        .ok()?;
        out.ok.then_some(out.stdout)
    }

    fn copy(&self, data: &[u8], mime: &str) -> Result<()> {
        feed(&self.wl_copy, &["--type", mime], data)
    }
}

impl Selection for WlClipboard {
    fn read_primary(&mut self) -> Option<String> {
        let types = self.types(true)?;
        let mime = TEXT_TYPES
            .iter()
            .find_map(|m| types.iter().find(|t| t.eq_ignore_ascii_case(m)))?;
        self.read(true, mime)
            .map(|data| String::from_utf8_lossy(&data).into_owned())
    }

    fn paste(&mut self, text: &str, keys: &mut dyn Keys) -> Result<()> {
        let saved = if self.restore {
            match self.types(false) {
                None => Some(Restore::Clear),
                Some(types) => Some(restore_plan(&types, is_gnome())),
            }
        } else {
            None
        };
        let saved = saved.map(|plan| match plan {
            Restore::Type(mime) => match self.read(false, &mime) {
                Some(data) => (Restore::Type(mime), data),
                None => (Restore::Clear, Vec::new()),
            },
            Restore::Clear => (Restore::Clear, Vec::new()),
        });
        self.copy(text.as_bytes(), UTF8)?;
        if !wait_until(|| self.read(false, UTF8), text.as_bytes()) {
            warn!("буфер обмена не обновился вовремя, вставляю всё равно");
        }
        keys.combo(&self.paste_keys)?;
        // Приложение забирает данные асинхронно — не подменять буфер раньше времени.
        thread::sleep(self.restore_delay);
        match saved {
            Some((Restore::Type(mime), data)) => self.copy(&data, &mime)?,
            Some((Restore::Clear, _)) => {
                debug!("буфер обмена очищен (был пуст или в нём был пароль)");
                let _ = run(&self.wl_copy, &["--clear"], TIMEOUT);
            }
            None => {}
        }
        Ok(())
    }
}

/// `wl-paste --primary --watch` печатает размер каждого нового выделения.
/// Возвращает причину остановки; `None` — служба завершается.
pub fn watch(tx: &Sender<Msg>) -> Option<String> {
    let started = Instant::now();
    let mut cmd = env::session_command("wl-paste");
    cmd.args(["--primary", "--watch", "sh", "-c", "wc -c"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // SAFETY: в pre_exec только async-signal-safe prctl: wl-paste завершится вместе со службой.
    unsafe {
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM as libc::c_ulong);
            Ok(())
        });
    }
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => return Some(format!("wl-paste: {err}")),
    };
    let mut stderr = child.stderr.take().expect("stderr");
    let errors = thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });
    let mut announced = false;
    for line in BufReader::new(child.stdout.take().expect("stdout")).lines() {
        let Ok(line) = line else { break };
        if !announced {
            announced = true;
            if tx.send(Msg::Selection(Ok("wl-clipboard"))).is_err() {
                let _ = child.kill();
                return None;
            }
        }
        if started.elapsed() < Duration::from_millis(700) {
            continue; // первое событие — уже существующее выделение
        }
        if let Ok(size) = line.trim().parse()
            && tx.send(Msg::Primary(size)).is_err()
        {
            let _ = child.kill();
            return None;
        }
    }
    let _ = child.wait();
    let err = errors.join().unwrap_or_default();
    Some(if err.trim().is_empty() {
        "wl-paste завершился".to_owned()
    } else {
        format!("wl-paste: {}", err.trim())
    })
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;

    use super::*;
    use crate::engine::Stroke;
    use crate::testutil::temp_dir;

    /// Фальшивые wl-paste/wl-copy: буферы — файлы `clip`/`primary` и их типы.
    fn fake_tools(dir: &Path) -> (String, String) {
        let paste = r#"#!/bin/sh
d="$(dirname "$0")"; sel=clip
[ "$1" = "--primary" ] && { sel=primary; shift; }
[ -f "$d/$sel" ] || { echo "Nothing is copied" >&2; exit 1; }
if [ "$1" = "--list-types" ]; then cat "$d/$sel.types"; exit 0; fi
[ "$1" = "--no-newline" ] && [ "$2" = "--type" ] || exit 2
grep -qx "$3" "$d/$sel.types" || { echo "no type $3" >&2; exit 1; }
cat "$d/$sel""#;
        let copy = r#"#!/bin/sh
d="$(dirname "$0")"
if [ "$1" = "--clear" ]; then rm -f "$d/clip" "$d/clip.types"; exit 0; fi
[ "$1" = "--type" ] || exit 2
cat > "$d/clip"; printf '%s\n' "$2" > "$d/clip.types""#;
        for (name, body) in [("wl-paste", paste), ("wl-copy", copy)] {
            let path = dir.join(name);
            fs::write(&path, body).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let tool = |n: &str| dir.join(n).to_string_lossy().into_owned();
        (tool("wl-paste"), tool("wl-copy"))
    }

    /// «Приложение»: на Ctrl+V читает текущий буфер обмена.
    struct App {
        dir: PathBuf,
        pasted: Rc<RefCell<Vec<String>>>,
    }

    impl Keys for App {
        fn backspace(&mut self, _: usize) -> Result<()> {
            Ok(())
        }
        fn type_strokes(&mut self, _: &[Stroke]) -> Result<()> {
            Ok(())
        }
        fn combo(&mut self, keys: &[KeyCode]) -> Result<()> {
            assert_eq!(keys, [KeyCode::KEY_LEFTCTRL, KeyCode::KEY_V]);
            self.pasted
                .borrow_mut()
                .push(fs::read_to_string(self.dir.join("clip"))?);
            Ok(())
        }
    }

    fn clipboard(dir: &Path) -> WlClipboard {
        let (paste, copy) = fake_tools(dir);
        let cfg = Config {
            paste_restore: Duration::ZERO,
            ..Config::default()
        };
        WlClipboard::new(&cfg, &paste, &copy)
    }

    #[test]
    fn reads_primary_text_only() {
        let dir = temp_dir("clip-primary");
        let mut clip = clipboard(&dir);
        assert_eq!(clip.read_primary(), None);
        fs::write(dir.join("primary"), "Ghbdtn").unwrap();
        fs::write(
            dir.join("primary.types"),
            "TEXT\ntext/plain;charset=utf-8\n",
        )
        .unwrap();
        assert_eq!(clip.read_primary().as_deref(), Some("Ghbdtn"));
        fs::write(dir.join("primary.types"), "image/png\n").unwrap();
        assert_eq!(clip.read_primary(), None);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn pastes_and_restores_previous_clipboard() {
        let dir = temp_dir("clip-paste");
        let mut clip = clipboard(&dir);
        fs::write(dir.join("clip"), [0x89, b'P', b'N', b'G']).unwrap();
        fs::write(dir.join("clip.types"), "image/png\n").unwrap();
        let pasted = Rc::new(RefCell::new(Vec::new()));
        let mut app = App {
            dir: dir.clone(),
            pasted: pasted.clone(),
        };
        clip.paste("Привет мир", &mut app).unwrap();
        assert_eq!(*pasted.borrow(), ["Привет мир"]);
        assert_eq!(
            fs::read(dir.join("clip")).unwrap(),
            [0x89, b'P', b'N', b'G']
        );
        assert_eq!(
            fs::read_to_string(dir.join("clip.types")).unwrap(),
            "image/png\n"
        );

        // Пустой буфер обмена после вставки снова пуст.
        fs::remove_file(dir.join("clip")).unwrap();
        clip.paste("ещё", &mut app).unwrap();
        assert_eq!(pasted.borrow().last().unwrap(), "ещё");
        assert!(!dir.join("clip").exists());

        // Пароль из менеджера паролей обратно не кладётся.
        fs::write(dir.join("clip"), "s3cret").unwrap();
        fs::write(
            dir.join("clip.types"),
            "text/plain;charset=utf-8\nx-kde-passwordManagerHint\n",
        )
        .unwrap();
        clip.paste("и ещё", &mut app).unwrap();
        assert_eq!(pasted.borrow().last().unwrap(), "и ещё");
        assert!(!dir.join("clip").exists(), "пароль не вернулся в буфер");
        fs::remove_dir_all(dir).unwrap();
    }
}
