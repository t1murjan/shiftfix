//! Wayland с протоколом data-control (KDE, Hyprland, Sway…): выделение
//! читается и вставляется через wl-clipboard.

use std::io::{BufRead, BufReader, Read};
use std::os::unix::process::CommandExt;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;

use super::clipboard::{Clipboard, Paste, TEXT_TYPES, UTF8};
use crate::config::Config;
use crate::daemon::Msg;
use crate::engine::{Keys, Selection};
use crate::sys::proc::{TIMEOUT, feed, run, session_command};

pub struct WlClipboard {
    paste: Paste,
    wl_paste: String,
    wl_copy: String,
}

impl WlClipboard {
    pub fn new(cfg: &Config, wl_paste: &str, wl_copy: &str) -> Self {
        WlClipboard {
            paste: Paste::new(cfg),
            wl_paste: wl_paste.into(),
            wl_copy: wl_copy.into(),
        }
    }

    fn list_types(&self, primary: bool) -> Option<Vec<String>> {
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

    fn read_from(&self, primary: bool, mime: &str) -> Option<Vec<u8>> {
        let base: &[&str] = if primary { &["--primary"] } else { &[] };
        let out = run(
            &self.wl_paste,
            &[base, &["--no-newline", "--type", mime][..]].concat(),
            TIMEOUT,
        )
        .ok()?;
        out.ok.then_some(out.stdout)
    }
}

impl Clipboard for WlClipboard {
    const TEXT: &str = UTF8;

    fn types(&self) -> Option<Vec<String>> {
        self.list_types(false)
    }

    fn read(&self, mime: &str) -> Option<Vec<u8>> {
        self.read_from(false, mime)
    }

    fn copy(&self, data: &[u8], mime: &str) -> Result<()> {
        feed(&self.wl_copy, &["--type", mime], data)
    }

    fn clear(&self) -> Result<()> {
        let _ = run(&self.wl_copy, &["--clear"], TIMEOUT);
        Ok(())
    }
}

impl Selection for WlClipboard {
    fn read_primary(&mut self) -> Option<String> {
        let types = self.list_types(true)?;
        let mime = TEXT_TYPES
            .iter()
            .find_map(|m| types.iter().find(|t| t.eq_ignore_ascii_case(m)))?;
        self.read_from(true, mime)
            .map(|data| String::from_utf8_lossy(&data).into_owned())
    }

    fn paste(&mut self, text: &str, keys: &mut dyn Keys) -> Result<()> {
        self.paste.run(self, text, keys)
    }
}

/// `wl-paste --primary --watch` печатает размер каждого нового выделения.
/// Возвращает причину остановки; `None` — служба завершается или слежение
/// остановлено (`stop`).
pub fn watch(tx: &Sender<Msg>, stop: &AtomicBool, wl_paste: &str) -> Option<String> {
    let started = Instant::now();
    let mut cmd = session_command(wl_paste);
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
        if stop.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
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
    use std::fs;
    use std::path::Path;

    use super::super::clipboard::testing::App;
    use super::*;
    use crate::testutil::{TempDir, write_script};

    /// Фальшивые wl-paste/wl-copy: буферы — файлы `clip`/`primary` и их типы.
    fn fake_tools(dir: &Path) -> (String, String) {
        let paste = r#"d="$(dirname "$0")"; sel=clip
[ "$1" = "--primary" ] && { sel=primary; shift; }
[ -f "$d/$sel" ] || { echo "Nothing is copied" >&2; exit 1; }
if [ "$1" = "--list-types" ]; then cat "$d/$sel.types"; exit 0; fi
[ "$1" = "--no-newline" ] && [ "$2" = "--type" ] || exit 2
grep -qx "$3" "$d/$sel.types" || { echo "no type $3" >&2; exit 1; }
cat "$d/$sel""#;
        let copy = r#"d="$(dirname "$0")"
if [ "$1" = "--clear" ]; then rm -f "$d/clip" "$d/clip.types"; exit 0; fi
[ "$1" = "--type" ] || exit 2
[ "$2" = image/bmp ] && { echo "не удалось" >&2; exit 1; }
cat > "$d/clip"; printf '%s\n' "$2" > "$d/clip.types""#;
        (
            write_script(dir, "wl-paste", paste),
            write_script(dir, "wl-copy", copy),
        )
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
        let dir = TempDir::new("clip-primary");
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
    }

    #[test]
    fn pastes_and_restores_previous_clipboard() {
        let dir = TempDir::new("clip-paste");
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

        // Пустой буфер обмена после вставки снова пуст.
        fs::remove_file(dir.join("clip")).unwrap();
        clip.paste("ещё", &mut app).unwrap();
        assert_eq!(app.pasted.last().unwrap(), "ещё");
        assert!(!dir.join("clip").exists());

        // Пароль из менеджера паролей обратно не кладётся.
        fs::write(dir.join("clip"), "s3cret").unwrap();
        fs::write(
            dir.join("clip.types"),
            "text/plain;charset=utf-8\nx-kde-passwordManagerHint\n",
        )
        .unwrap();
        clip.paste("и ещё", &mut app).unwrap();
        assert_eq!(app.pasted.last().unwrap(), "и ещё");
        assert!(!dir.join("clip").exists(), "пароль не вернулся в буфер");
    }

    #[test]
    fn restores_clipboard_when_paste_fails() {
        let dir = TempDir::new("clip-paste-fails");
        let mut clip = clipboard(&dir);
        fs::write(dir.join("clip"), "было").unwrap();
        fs::write(dir.join("clip.types"), format!("{UTF8}\n")).unwrap();
        let mut app = App::new(&dir);
        app.broken = true;
        assert!(clip.paste("стало", &mut app).is_err());
        assert_eq!(fs::read_to_string(dir.join("clip")).unwrap(), "было");
    }

    #[test]
    fn failed_restore_does_not_fail_paste() {
        // Текст уже вставлен: если прежний буфер не вернуть, движок всё равно
        // должен переключить раскладку и запомнить исправление.
        let dir = TempDir::new("clip-restore-fails");
        let mut clip = clipboard(&dir);
        fs::write(dir.join("clip"), "BM").unwrap();
        fs::write(dir.join("clip.types"), "image/bmp\n").unwrap();
        let mut app = App::new(&dir);
        clip.paste("Привет", &mut app).unwrap();
        assert_eq!(app.pasted, ["Привет"]);
    }

    #[test]
    fn watch_stops_on_request() {
        let dir = TempDir::new("clip-watch");
        let wl_paste = write_script(&dir, "wl-paste", "while :; do echo 3; sleep 0.05; done");
        let (tx, rx) = std::sync::mpsc::channel();
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let (done_tx, done) = std::sync::mpsc::channel();
        let flag = std::sync::Arc::clone(&stop);
        thread::spawn(move || done_tx.send(watch(&tx, &flag, &wl_paste)));
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(5)),
            Ok(Msg::Selection(Ok("wl-clipboard")))
        ));
        stop.store(true, Ordering::Relaxed);
        assert_eq!(done.recv_timeout(Duration::from_secs(5)), Ok(None));
    }
}
