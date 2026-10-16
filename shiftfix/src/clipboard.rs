//! Чтение PRIMARY и вставка через буфер обмена: wl-clipboard (Wayland с
//! протоколом data-control) или xclip (X11 и XWayland).
//!
//! Перед вставкой буфер обмена сохраняется и потом возвращается. Вернуть
//! можно только один формат, поэтому он выбирается с умом: пароли из
//! менеджера паролей не возвращаются вовсе (иначе они попадут в историю
//! Klipper и подобных), скопированные файлы возвращаются как файлы.

use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use evdev::KeyCode;
use log::{debug, warn};

use crate::config::Config;
use crate::engine::{Keys, Selection};
use crate::env;
use crate::proc::{TIMEOUT, feed, run};

const TEXT_TYPES: [&str; 5] = [
    "text/plain;charset=utf-8",
    "utf8_string",
    "text/plain",
    "string",
    "text",
];
const UTF8: &str = "text/plain;charset=utf-8";
/// Отметка менеджеров паролей (KeePassXC, Bitwarden…): не сохранять в истории.
const SECRET: &str = "x-kde-passwordmanagerhint";
/// Признаки скопированных файлов (Dolphin, Nautilus, Thunar…).
const FILE_MARKERS: [&str; 3] = [
    "x-special/gnome-copied-files",
    "application/x-kde4-urilist",
    "application/x-kde-cutselection",
];

/// Что вернуть в буфер обмена после вставки.
#[derive(Debug, PartialEq, Eq)]
pub enum Restore {
    /// Буфер был пуст или в нём пароль — очистить.
    Clear,
    /// Вернуть данные этого типа.
    Type(String),
}

/// Выбрать тип для возврата по списку типов буфера обмена.
pub fn restore_plan(types: &[String], gnome: bool) -> Restore {
    let find = |wanted: &str| types.iter().find(|t| t.eq_ignore_ascii_case(wanted));
    if find(SECRET).is_some() {
        return Restore::Clear;
    }
    if FILE_MARKERS.iter().any(|m| find(m).is_some()) {
        let preferred = if gnome {
            ["x-special/gnome-copied-files", "text/uri-list"]
        } else {
            ["text/uri-list", "x-special/gnome-copied-files"]
        };
        if let Some(t) = preferred.iter().find_map(|m| find(m)) {
            return Restore::Type(t.clone());
        }
    }
    let useful = |t: &&String| {
        let t = t.to_ascii_lowercase();
        // Служебные цели X11 — не данные.
        !matches!(
            t.as_str(),
            "targets" | "timestamp" | "multiple" | "save_targets" | "delete" | "insert_selection"
        )
    };
    TEXT_TYPES
        .iter()
        .find_map(|m| find(m))
        .or_else(|| find("image/png"))
        .or_else(|| types.iter().find(useful))
        .map_or(Restore::Clear, |t| Restore::Type(t.clone()))
}

fn is_gnome() -> bool {
    env::desktop().contains("gnome")
}

/// Подождать, пока буфер обмена действительно получит `data`: копирующая
/// программа уходит в фон раньше, чем композитор объявит новое содержимое.
fn wait_until(read: impl Fn() -> Option<Vec<u8>>, data: &[u8]) -> bool {
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        if read().is_some_and(|got| got == data) {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    false
}

// ─── Wayland: wl-clipboard ──────────────────────────────────────────────────

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

// ─── X11 и XWayland: xclip ──────────────────────────────────────────────────

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

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;

    use super::*;
    use crate::engine::Stroke;

    fn types(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn chooses_what_to_restore() {
        // KeePassXC: пароль не возвращаем.
        let secret = types(&[UTF8, "text/plain", "x-kde-passwordManagerHint"]);
        assert_eq!(restore_plan(&secret, false), Restore::Clear);
        // Файлы из Dolphin — как файлы, а не как текст с путями.
        let dolphin = types(&[
            "application/x-kde4-urilist",
            "text/uri-list",
            UTF8,
            "text/plain",
        ]);
        assert_eq!(
            restore_plan(&dolphin, false),
            Restore::Type("text/uri-list".into())
        );
        let nautilus = types(&["x-special/gnome-copied-files", "text/uri-list", UTF8]);
        assert_eq!(
            restore_plan(&nautilus, true),
            Restore::Type("x-special/gnome-copied-files".into())
        );
        // Ссылка из браузера — это текст.
        let link = types(&["text/x-moz-url", "text/uri-list", "text/html", UTF8]);
        assert_eq!(restore_plan(&link, false), Restore::Type(UTF8.into()));
        // X11: служебные цели пропускаются.
        let x11 = types(&["TIMESTAMP", "TARGETS", "MULTIPLE", "UTF8_STRING", "STRING"]);
        assert_eq!(
            restore_plan(&x11, false),
            Restore::Type("UTF8_STRING".into())
        );
        let image = types(&["TARGETS", "image/bmp", "image/png"]);
        assert_eq!(
            restore_plan(&image, false),
            Restore::Type("image/png".into())
        );
        assert_eq!(restore_plan(&types(&["TARGETS"]), false), Restore::Clear);
        assert_eq!(restore_plan(&[], false), Restore::Clear);
    }

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
