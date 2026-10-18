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
use std::thread;
use std::time::Duration;

use eframe::egui;
use shiftfix::config::ThemeMode;
use shiftfix::sys::{env, lock, proc};

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
    env::runtime_dir().join("shiftfix-gui.sock")
}

/// Попросить уже открытое окно показаться; `false` — оно не ответило.
fn raise_existing() -> bool {
    UnixStream::connect(raise_socket())
        .and_then(|mut stream| stream.write_all(b"show\n"))
        .is_ok()
}

/// Показывать окно, когда его открывают ещё раз (значок в трее, ярлык в меню).
fn listen_for_raise(ctx: egui::Context) {
    let path = raise_socket();
    let _ = fs::remove_file(&path); // блокировка у нас — старый сокет ничей
    let Ok(listener) = UnixListener::bind(&path) else {
        return;
    };
    let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    let spawned = thread::Builder::new().name("raise".into()).spawn(move || {
        for stream in listener.incoming().flatten() {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
            let mut line = String::new();
            if BufReader::new(stream).read_line(&mut line).is_ok() && line.trim() == "show" {
                // В Wayland композитор может вместо этого подсветить окно на панели.
                let root = egui::ViewportId::ROOT;
                ctx.send_viewport_cmd_to(root, egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd_to(root, egui::ViewportCommand::Focus);
                ctx.request_repaint();
            }
        }
    });
    if let Err(err) = spawned {
        eprintln!("shiftfix-gui: {err}");
    }
}

fn main() -> eframe::Result {
    shiftfix::logbuf::init(false);
    let mut page = app::Page::Home;
    let mut forced_theme = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--page" => match args.next().as_deref().and_then(app::Page::parse) {
                Some(p) => page = p,
                None => {
                    eprintln!("{USAGE}");
                    std::process::exit(2);
                }
            },
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
    let _lock = match lock::try_lock(&env::runtime_dir().join("shiftfix-gui.lock")) {
        Ok(Some(file)) => Some(file),
        Ok(None) => {
            if !raise_existing() {
                report("окно shiftfix уже открыто", false);
            }
            return Ok(());
        }
        // Файл блокировки не создать — открываемся без него.
        Err(_) => None,
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
            listen_for_raise(cc.egui_ctx.clone());
            theme::install_fonts(&cc.egui_ctx);
            theme::apply_style(&cc.egui_ctx);
            system.attach(&cc.egui_ctx);
            Ok(Box::new(app::App::new(cc, page, forced_theme, system)))
        }),
    );
    if let Err(err) = &result {
        report(&format!("окно не открылось: {err}"), true);
    }
    result
}
