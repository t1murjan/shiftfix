//! Выделение (PRIMARY) и вставка через буфер обмена — wl-clipboard.

use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use evdev::KeyCode;
use log::warn;

use crate::config::Config;
use crate::engine::{Keys, Selection};
use crate::env::in_path;
use crate::proc::{TIMEOUT, feed, run};

const TEXT_MIMES: [&str; 5] = [
    "text/plain;charset=utf-8",
    "text/plain",
    "utf8_string",
    "string",
    "text",
];
const UTF8: &str = "text/plain;charset=utf-8";

pub struct WlClipboard {
    paste_keys: Vec<KeyCode>,
    restore: bool,
    restore_delay: Duration,
    wl_paste: String,
    wl_copy: String,
}

/// `None`, если исправлять выделение нельзя или не нужно.
pub fn make(cfg: &Config) -> Option<Box<dyn Selection>> {
    if !cfg.selection {
        return None;
    }
    // Сессия Wayland может появиться позже — её переменные подставит proc::run.
    let missing: Vec<&str> = ["wl-paste", "wl-copy"]
        .into_iter()
        .filter(|t| !in_path(t))
        .collect();
    if !missing.is_empty() {
        warn!(
            "нет {} — выделение исправляться не будет (установи wl-clipboard)",
            missing.join(", ")
        );
        return None;
    }
    Some(Box::new(WlClipboard::new(cfg, "wl-paste", "wl-copy")))
}

impl WlClipboard {
    fn new(cfg: &Config, wl_paste: &str, wl_copy: &str) -> Self {
        WlClipboard {
            paste_keys: cfg.paste_keys.clone(),
            restore: cfg.restore_clipboard,
            restore_delay: cfg.paste_restore,
            wl_paste: wl_paste.into(),
            wl_copy: wl_copy.into(),
        }
    }

    /// Содержимое PRIMARY или буфера обмена: (MIME-тип, данные).
    fn get(&self, primary: bool, text_only: bool) -> Option<(String, Vec<u8>)> {
        let base: &[&str] = if primary { &["--primary"] } else { &[] };
        let list = run(
            &self.wl_paste,
            &[base, &["--list-types"][..]].concat(),
            Duration::from_secs(1),
        )
        .ok()?;
        if !list.ok {
            return None; // «Nothing is copied»
        }
        let types: Vec<String> = String::from_utf8_lossy(&list.stdout)
            .split_whitespace()
            .map(String::from)
            .collect();
        let text_type = TEXT_MIMES
            .iter()
            .find_map(|m| types.iter().find(|t| t.to_lowercase() == *m));
        let mime = match text_type {
            Some(t) => t.clone(),
            None if text_only => return None,
            None => types.first()?.clone(),
        };
        let out = run(
            &self.wl_paste,
            &[base, &["--no-newline", "--type", mime.as_str()][..]].concat(),
            TIMEOUT,
        )
        .ok()?;
        out.ok.then_some((mime, out.stdout))
    }

    fn copy(&self, data: &[u8], mime: &str) -> Result<()> {
        feed(&self.wl_copy, &["--type", mime], data)
    }

    /// wl-copy уходит в фон раньше, чем композитор объявит новое содержимое.
    fn wait_for(&self, data: &[u8]) -> bool {
        let deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < deadline {
            let out = run(
                &self.wl_paste,
                &["--no-newline", "--type", UTF8],
                Duration::from_secs(1),
            );
            if out.is_ok_and(|o| o.ok && o.stdout == data) {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        false
    }
}

impl Selection for WlClipboard {
    fn read_primary(&mut self) -> Option<String> {
        self.get(true, true)
            .map(|(_, data)| String::from_utf8_lossy(&data).into_owned())
    }

    fn paste(&mut self, text: &str, keys: &mut dyn Keys) -> Result<()> {
        let saved = if self.restore {
            self.get(false, false)
        } else {
            None
        };
        self.copy(text.as_bytes(), UTF8)?;
        if !self.wait_for(text.as_bytes()) {
            warn!("буфер обмена не обновился вовремя, вставляю всё равно");
        }
        keys.combo(&self.paste_keys)?;
        // Приложение забирает данные асинхронно — не подменять буфер раньше времени.
        thread::sleep(self.restore_delay);
        if self.restore {
            match saved {
                Some((mime, data)) => self.copy(&data, &mime)?,
                None => {
                    let _ = run(&self.wl_copy, &["--clear"], TIMEOUT);
                }
            }
        }
        Ok(())
    }
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

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("shiftfix-clip-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
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
        let dir = temp_dir("primary");
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
        let dir = temp_dir("paste");
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
        fs::remove_dir_all(dir).unwrap();
    }
}
