//! Раскладка XKB, keymap и выделение на настоящем X-сервере (Xvfb).

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use anyhow::Result;
use evdev::KeyCode;
use shiftfix::backend::XkbLayout;
use shiftfix::daemon::Msg;
use shiftfix::engine::{Backend, Keys};
use shiftfix::keys::Stroke;
use shiftfix::sys::{env, paths};
use shiftfix::{keymap, selection};

/// Пропуск теста, которому не хватает программы или библиотеки; с
/// `SHIFTFIX_TEST_STRICT=1` (так в CI) — падение. Копия `testutil::skip`
/// библиотеки: её тестовый код (`cfg(test)`) интеграционным тестам недоступен.
fn skip(reason: &str) {
    if std::env::var_os("SHIFTFIX_TEST_STRICT").is_some_and(|v| v == "1") {
        panic!("{reason}, а пропуск запрещён: SHIFTFIX_TEST_STRICT=1");
    }
    eprintln!("— пропуск: {reason}");
}

struct NoKeys;

impl Keys for NoKeys {
    fn backspace(&mut self, _: usize) -> Result<()> {
        Ok(())
    }

    fn type_strokes(&mut self, _: &[Stroke]) -> Result<()> {
        Ok(())
    }

    fn combo(&mut self, _: &[KeyCode]) -> Result<()> {
        Ok(())
    }

    fn chord(&mut self, _: &[KeyCode], _: KeyCode, _: usize) -> Result<()> {
        Ok(())
    }
}

/// Xvfb на время теста; при выходе, в том числе при панике, — SIGTERM и wait.
struct Xvfb {
    child: Child,
    display: String,
}

impl Drop for Xvfb {
    fn drop(&mut self) {
        // SIGTERM, а не kill(): по нему Xvfb убирает свой сокет в /tmp/.X11-unix,
        // иначе поиск X-сервера потом принимает оставшийся сокет за живой.
        // SAFETY: pid нашего дочернего процесса, wait ещё не вызывался.
        unsafe { libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM) };
        let _ = self.child.wait();
    }
}

/// Xvfb на свободном дисплее. Номер он пишет в `-displayfd`, когда уже слушает
/// сокет: по нему X-сервер находит библиотека. `None` — не запустился.
fn start_xvfb() -> Option<Xvfb> {
    let child = Command::new("Xvfb")
        .args(["-displayfd", "1", "-nolisten", "tcp", "-noreset"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut server = Xvfb {
        child,
        display: String::new(),
    };
    let stdout = server.child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = BufReader::new(stdout).read_line(&mut line);
        let _ = tx.send(line);
    });
    let line = rx.recv_timeout(Duration::from_secs(10)).ok()?;
    let number: u32 = line.trim().parse().ok()?;
    server.display = format!(":{number}");
    Some(server)
}

fn setxkbmap(display: &str, layouts: &str) {
    let status = Command::new("setxkbmap")
        .args(["-layout", layouts])
        .env("DISPLAY", display)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "setxkbmap -layout {layouts}: {status}");
}

/// XkbLayout: список раскладок, текущая, set и toggle.
fn switches_layouts() {
    let mut xkb = XkbLayout::default();
    let mut keys = NoKeys;
    assert_eq!(xkb.layouts().unwrap(), ["us", "ru"]);
    assert_eq!(xkb.current().unwrap(), Some(0));
    xkb.set(1, &mut keys).unwrap();
    assert_eq!(xkb.current().unwrap(), Some(1));
    xkb.toggle(&mut keys).unwrap();
    assert_eq!(xkb.current().unwrap(), Some(0));
    assert!(xkb.set(4, &mut keys).is_err());
}

/// Keymap приходит и без слежения за выделением, и после смены раскладок.
fn keymap_follows_layouts(display: &str) {
    if keymap::from_names("us", "", "").is_none() {
        return skip("нет libxkbcommon — keymap X11 не проверяется");
    }
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        keymap::x11::watch(&|keymap| {
            let _ = tx.send(keymap.layouts.len());
        })
    });
    let layouts = || {
        rx.recv_timeout(Duration::from_secs(5))
            .expect("keymap от X11")
    };
    assert_eq!(layouts(), 2);
    setxkbmap(display, "us,ru,de");
    assert_eq!(layouts(), 3);
}

/// Слежение за PRIMARY: сначала «работаю через X11», на новое выделение —
/// Primary; после остановки поток выходит на следующем событии.
fn reports_primary(display: &str) {
    let (tx, rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let (done_tx, done) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = done_tx.send(selection::x11::watch(&tx, &flag).is_ok());
    });
    let next = || {
        rx.recv_timeout(Duration::from_secs(5))
            .expect("событие от X11")
    };
    assert!(
        matches!(next(), Msg::Selection(Ok("X11"))),
        "ждали Selection(Ok(\"X11\")), keymap выделение больше не присылает"
    );
    let mut xclip = Command::new("xclip")
        .args(["-selection", "primary", "-i"])
        .env("DISPLAY", display)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    drop(xclip.stdin.take()); // пустой ввод тоже делает xclip владельцем PRIMARY
    let primary = match next() {
        Msg::Primary(size) => size,
        _ => panic!("ждали Primary"),
    };
    assert_eq!(primary, 1);
    let _ = xclip.kill();
    let _ = xclip.wait();
    stop.store(true, Ordering::Relaxed);
    let owned = Command::new("xclip")
        .args(["-selection", "primary", "-i", "/dev/null"])
        .env("DISPLAY", display)
        .status()
        .unwrap();
    assert!(owned.success());
    assert_eq!(done.recv_timeout(Duration::from_secs(5)), Ok(true));
}

/// Раскладки XKB и PRIMARY на своём Xvfb. Пропускается, если Xvfb, setxkbmap
/// или xclip нет, а также при живом X-сеансе: тест не должен переключать
/// раскладку на машине разработчика.
#[test]
fn xkb_and_primary_on_xvfb() {
    if !["Xvfb", "setxkbmap", "xclip"]
        .iter()
        .all(|t| paths::in_path(t))
    {
        return skip("нет Xvfb, setxkbmap или xclip");
    }
    let Some(xvfb) = start_xvfb() else {
        return skip("Xvfb не запустился");
    };
    // Библиотека выбирает X-сервер сама (env::x11_display). Выбрала другой —
    // значит, есть живой сеанс: это не нехватка программ, пропуск и в строгом
    // режиме. Не видит никакого — сломан сам поиск.
    match env::x11_display() {
        Some(display) if display == xvfb.display => {}
        Some(other) => {
            eprintln!("— пропуск: есть живой X-сервер {other}");
            return;
        }
        None => panic!("env::x11_display() не видит Xvfb {}", xvfb.display),
    }
    setxkbmap(&xvfb.display, "us,ru");
    switches_layouts();
    keymap_follows_layouts(&xvfb.display);
    reports_primary(&xvfb.display);
}
