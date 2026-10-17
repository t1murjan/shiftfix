//! Раскладка XKB, keymap и выделение на настоящем X-сервере (Xvfb).

use anyhow::Result;
use evdev::KeyCode;
use shiftfix::backend::XkbLayout;
use shiftfix::daemon::Msg;
use shiftfix::engine::{Backend, Keys, Stroke};
use shiftfix::sys::env;
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
}

/// Раскладки XKB и PRIMARY на настоящем X-сервере (Xvfb). Пропускается, если
/// Xvfb, setxkbmap или xclip нет, а также при живом X-сеансе: тест не должен
/// переключать раскладку на машине разработчика.
#[test]
fn xkb_and_primary_on_xvfb() {
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    const DISPLAY: &str = ":97";
    if !["Xvfb", "setxkbmap", "xclip"]
        .iter()
        .all(|t| env::in_path(t))
    {
        return skip("нет Xvfb, setxkbmap или xclip");
    }
    if env::x11_display().is_some() {
        // Не нехватка программ, а защита живого сеанса: пропуск и в строгом режиме.
        eprintln!("— пропуск: есть живой X-сервер");
        return;
    }
    let Ok(mut server) = Command::new("Xvfb")
        .args([DISPLAY, "-nolisten", "tcp", "-noreset"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return skip("Xvfb не запустился");
    };
    struct Kill(std::process::Child);
    impl Drop for Kill {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let started = Instant::now();
    while env::x11_display().as_deref() != Some(DISPLAY) {
        if started.elapsed() > Duration::from_secs(5) {
            let _ = server.kill();
            return skip("Xvfb не поднялся");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _server = Kill(server);
    let x = |program: &str, args: &[&str]| {
        Command::new(program)
            .args(args)
            .env("DISPLAY", DISPLAY)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .status()
            .unwrap()
    };
    assert!(x("setxkbmap", &["-layout", "us,ru"]).success());

    let mut xkb = XkbLayout::default();
    let mut keys = NoKeys;
    assert_eq!(xkb.layouts().unwrap(), ["us", "ru"]);
    assert_eq!(xkb.current().unwrap(), Some(0));
    xkb.set(1, &mut keys).unwrap();
    assert_eq!(xkb.current().unwrap(), Some(1));
    xkb.toggle(&mut keys).unwrap();
    assert_eq!(xkb.current().unwrap(), Some(0));
    assert!(xkb.set(4, &mut keys).is_err());

    // Keymap приходит и без слежения за выделением, и после смены раскладок.
    if keymap::from_names("us", "", "").is_some() {
        let (ktx, krx) = mpsc::channel();
        std::thread::spawn(move || {
            keymap::x11::watch(&|keymap| {
                let _ = ktx.send(keymap.layouts.len());
            })
        });
        let layouts = || {
            krx.recv_timeout(Duration::from_secs(5))
                .expect("keymap от X11")
        };
        assert_eq!(layouts(), 2);
        assert!(x("setxkbmap", &["-layout", "us,ru,de"]).success());
        assert_eq!(layouts(), 3);
    } else {
        skip("нет libxkbcommon — keymap X11 не проверяется");
    }

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || selection::x11::watch(&tx));
    let next = || {
        rx.recv_timeout(Duration::from_secs(5))
            .expect("событие от X11")
    };
    assert!(
        matches!(next(), Msg::Selection(Ok("X11"))),
        "ждали Selection(Ok(\"X11\")), keymap выделение больше не присылает"
    );
    // Новое выделение — событие Primary.
    let mut xclip = Command::new("xclip")
        .args(["-selection", "primary", "-i"])
        .env("DISPLAY", DISPLAY)
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
}
