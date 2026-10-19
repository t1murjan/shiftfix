//! `shiftfix-gui` — графическая оболочка: состояние службы, устройства,
//! настройки, инструкция и журнал.

mod app;
mod edit;
mod format;
mod link;
mod pages;
mod portal;
mod theme;
mod toast;
mod widgets;

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use eframe::egui;
use shiftfix::config::ThemeMode;
use shiftfix::sys::{lock, paths, proc};

const USAGE: &str = "\
shiftfix-gui — настройки и состояние shiftfix

Использование: shiftfix-gui [--page home|devices|settings|help|log] [--theme system|light|dark]

  --page          открыть вкладку
  --theme         режим оформления только на этот запуск (в настройки не пишется)
  -V, --version   версия
  -h, --help      эта справка
";

/// Окно создано: паника после этого — ошибка в работающем окне, а не при запуске.
static SHOWN: AtomicBool = AtomicBool::new(false);

/// Сообщить и там, где видно без терминала: окно запускают из меню или трея,
/// и stderr туда не попадает. Критическое уведомление само не исчезает.
fn report(text: &str, critical: bool) {
    eprintln!("shiftfix-gui: {text}");
    let urgency = if critical { "critical" } else { "normal" };
    proc::notify(text, &["-u", urgency]);
}

/// Сокет, через который второй запуск просит открытое окно показаться.
fn raise_socket() -> PathBuf {
    paths::runtime_dir().join("shiftfix-gui.sock")
}

/// Просьба показаться: `show` и, если вкладку назвали в `--page`, её имя.
fn raise_line(page: Option<app::Page>) -> String {
    match page {
        Some(page) => format!("show {}\n", page.name()),
        None => "show\n".to_owned(),
    }
}

/// Разбор просьбы из сокета: `None` — это не она, `Some(None)` — показаться,
/// не меняя вкладку. Незнакомая вкладка (от другой версии) — тоже просто показаться.
fn parse_raise(line: &str) -> Option<Option<app::Page>> {
    let mut words = line.split_whitespace();
    if words.next() != Some("show") {
        return None;
    }
    Some(words.next().and_then(app::Page::parse))
}

/// Попросить уже открытое окно показаться и открыть вкладку `page`;
/// `false` — оно не ответило.
fn raise_existing(page: Option<app::Page>) -> bool {
    UnixStream::connect(raise_socket())
        .and_then(|mut stream| stream.write_all(raise_line(page).as_bytes()))
        .is_ok()
}

/// Показывать окно, когда его открывают ещё раз (значок в трее, ярлык в меню).
/// Вкладки, которые при этом просят открыть, приходят в возвращённый канал.
fn listen_for_raise(ctx: egui::Context) -> Receiver<app::Page> {
    let (pages, raised) = mpsc::channel();
    let path = raise_socket();
    let _ = fs::remove_file(&path); // блокировка у нас — старый сокет ничей
    let listener = match UnixListener::bind(&path) {
        Ok(listener) => listener,
        Err(err) => {
            log::warn!(
                "сокет {}: {err} — повторный запуск не сможет показать окно",
                path.display()
            );
            return raised;
        }
    };
    let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    let spawned = thread::Builder::new().name("raise".into()).spawn(move || {
        for stream in listener.incoming().flatten() {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
            let mut line = String::new();
            if BufReader::new(stream).read_line(&mut line).is_err() {
                continue;
            }
            let Some(page) = parse_raise(&line) else {
                continue;
            };
            if let Some(page) = page {
                let _ = pages.send(page);
            }
            // В Wayland композитор может вместо этого подсветить окно на панели.
            let root = egui::ViewportId::ROOT;
            ctx.send_viewport_cmd_to(root, egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd_to(root, egui::ViewportCommand::Focus);
            ctx.request_repaint();
        }
    });
    if let Err(err) = spawned {
        log::warn!("сокет {}: поток не запущен: {err}", path.display());
    }
    raised
}

fn main() -> eframe::Result {
    shiftfix::logbuf::init(false);
    let mut page = None;
    let mut forced_theme = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--page" => {
                let name = args.next().unwrap_or_default();
                match app::Page::parse(name.trim()) {
                    Some(p) => page = Some(p),
                    None => {
                        eprintln!("shiftfix-gui: нет вкладки «{name}»\n\n{USAGE}");
                        std::process::exit(2);
                    }
                }
            }
            "--theme" => match args
                .next()
                .as_deref()
                .map(str::trim)
                .and_then(ThemeMode::parse)
            {
                Some(t) => forced_theme = Some(t),
                None => {
                    eprintln!("{USAGE}");
                    std::process::exit(2);
                }
            },
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(());
            }
            // До блокировки и окна: работает и при открытом окне, и без графики.
            "-V" | "--version" => {
                println!("shiftfix-gui {}", app::VERSION);
                return Ok(());
            }
            other => {
                eprintln!("shiftfix-gui: неизвестный параметр {other}\n\n{USAGE}");
                std::process::exit(2);
            }
        }
    }

    // winit паникует, а не возвращает ошибку, когда нет нужной библиотеки
    // (например, libxkbcommon-x11): об этом тоже надо сказать.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default_hook(info);
        let reason = info.payload_as_str().unwrap_or("внутренняя ошибка");
        if SHOWN.load(Ordering::Relaxed) {
            report(&format!("ошибка в окне: {reason}"), false);
        } else {
            report(&format!("окно не открылось: {reason}"), true);
        }
    }));

    // Второе окно рядом с первым только путает: настройки сохранялись бы наперегонки.
    let lock_path = paths::runtime_dir().join("shiftfix-gui.lock");
    let _lock = match lock::try_lock(&lock_path) {
        Ok(Some(file)) => Some(file),
        Ok(None) => {
            if !raise_existing(page) {
                report("окно shiftfix уже открыто", false);
            }
            return Ok(());
        }
        Err(err) => {
            log::warn!(
                "блокировка окна {}: {err} — открываюсь без неё",
                lock_path.display()
            );
            None
        }
    };

    // Портал отвечает не сразу: спрашиваем, пока создаётся окно.
    let system = theme::SystemWatch::start();
    let icon = egui::IconData {
        rgba: shiftfix::icon::rgba(128, true),
        width: 128,
        height: 128,
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("shiftfix")
            .with_app_id("shiftfix")
            .with_inner_size([1000.0, 720.0])
            .with_min_inner_size([780.0, 540.0])
            .with_icon(Arc::new(icon)),
        ..Default::default()
    };
    let result = eframe::run_native(
        "shiftfix",
        options,
        Box::new(move |cc| {
            SHOWN.store(true, Ordering::Relaxed);
            let raised = listen_for_raise(cc.egui_ctx.clone());
            theme::install_fonts(&cc.egui_ctx);
            theme::apply_style(&cc.egui_ctx);
            system.attach(&cc.egui_ctx);
            let page = page.unwrap_or(app::Page::Home);
            Ok(Box::new(app::App::new(
                cc,
                page,
                raised,
                forced_theme,
                system,
            )))
        }),
    );
    if let Err(err) = &result {
        report(&format!("окно не открылось: {err}"), true);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raise_carries_page() {
        for page in app::Page::ALL {
            assert_eq!(parse_raise(&raise_line(Some(page))), Some(Some(page)));
        }
        assert_eq!(parse_raise(&raise_line(None)), Some(None));
        // Вкладка из другой версии окна — показаться, вкладку не трогать.
        assert_eq!(parse_raise("show nope\n"), Some(None));
        assert_eq!(parse_raise("hide\n"), None);
        assert_eq!(parse_raise(""), None);
    }
}
