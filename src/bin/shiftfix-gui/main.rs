//! `shiftfix-gui` — графическая оболочка: состояние службы, устройства,
//! настройки, инструкция и журнал.

mod app;
mod edit;
mod format;
mod instance;
mod link;
mod pages;
mod portal;
mod theme;
mod toast;
mod update;
mod widgets;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};

use anyhow::{Result, bail};
use eframe::egui;
use shiftfix::config::ThemeMode;
use shiftfix::sys::proc;

use instance::{Instance, Start};

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

#[derive(Debug, PartialEq, Eq)]
enum Command {
    /// Открыть окно: вкладка из `--page`, режим оформления из `--theme`.
    Run {
        page: Option<app::Page>,
        theme: Option<ThemeMode>,
    },
    Help,
    Version,
}

/// Параметры без имени программы.
fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Command> {
    let mut it = args.into_iter();
    let (mut page, mut theme) = (None, None);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--page" => {
                let name = it.next().unwrap_or_default();
                let Some(p) = app::Page::parse(name.trim()) else {
                    bail!("нет вкладки «{name}»");
                };
                page = Some(p);
            }
            "--theme" => {
                let name = it.next().unwrap_or_default();
                let Some(t) = ThemeMode::parse(name.trim()) else {
                    bail!("нет темы «{name}»");
                };
                theme = Some(t);
            }
            "-h" | "--help" => return Ok(Command::Help),
            "-V" | "--version" => return Ok(Command::Version),
            other => bail!("неизвестный параметр {other}"),
        }
    }
    Ok(Command::Run { page, theme })
}

fn main() -> eframe::Result {
    shiftfix::logbuf::init(false);
    match parse_args(std::env::args().skip(1)) {
        Ok(Command::Run { page, theme }) => run_window(page, theme),
        Ok(Command::Help) => {
            print!("{USAGE}");
            Ok(())
        }
        // До блокировки и окна: работает и при открытом окне, и без графики.
        Ok(Command::Version) => {
            println!("shiftfix-gui {}", app::VERSION);
            Ok(())
        }
        Err(err) => {
            eprintln!("shiftfix-gui: {err}\n\n{USAGE}");
            std::process::exit(2);
        }
    }
}

/// Окно с вкладками. Второе окно рядом с первым только путает: настройки
/// сохранялись бы наперегонки, — поэтому второй запуск просит открытое окно
/// показаться и открыть вкладку из `--page`.
fn run_window(page: Option<app::Page>, forced_theme: Option<ThemeMode>) -> eframe::Result {
    let _lock = match Instance::Window.start() {
        Start::First(lock) => lock,
        Start::Second => {
            if !Instance::Window.send(&instance::raise_line(page)) {
                report("окно shiftfix уже открыто", false);
            }
            return Ok(());
        }
    };
    let viewport = egui::ViewportBuilder::default()
        .with_title("shiftfix")
        .with_app_id("shiftfix")
        .with_inner_size([1000.0, 720.0])
        .with_min_inner_size([780.0, 540.0]);
    launch(viewport, move |cc, system| {
        let raised = listen_for_raise(cc.egui_ctx.clone());
        let page = page.unwrap_or(app::Page::Home);
        Box::new(app::App::new(cc, page, raised, forced_theme, system))
    })
}

/// Показывать окно, когда его открывают ещё раз (значок в трее, ярлык в меню).
/// Вкладки, которые при этом просят открыть, приходят в возвращённый канал.
fn listen_for_raise(ctx: egui::Context) -> Receiver<app::Page> {
    let (pages, raised) = mpsc::channel();
    Instance::Window.listen(move |line| {
        let Some(page) = instance::parse_raise(line) else {
            return;
        };
        if let Some(page) = page {
            let _ = pages.send(page);
        }
        // В Wayland композитор может вместо этого подсветить окно на панели.
        let root = egui::ViewportId::ROOT;
        ctx.send_viewport_cmd_to(root, egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd_to(root, egui::ViewportCommand::Focus);
        ctx.request_repaint();
    });
    raised
}

/// Открыть окно `viewport` со значком, шрифтами, стилем и темой системы;
/// `make` создаёт то, что в нём рисуется. Ошибку и панику показать и без
/// терминала: окно запускают из меню или трея.
fn launch(
    viewport: egui::ViewportBuilder,
    make: impl FnOnce(&eframe::CreationContext<'_>, theme::SystemWatch) -> Box<dyn eframe::App>
    + 'static,
) -> eframe::Result {
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

    // Портал отвечает не сразу: спрашиваем, пока создаётся окно.
    let system = theme::SystemWatch::start();
    let icon = egui::IconData {
        rgba: shiftfix::icon::rgba(128, true),
        width: 128,
        height: 128,
    };
    let options = eframe::NativeOptions {
        viewport: viewport.with_icon(Arc::new(icon)),
        ..Default::default()
    };
    let result = eframe::run_native(
        "shiftfix",
        options,
        Box::new(move |cc| {
            SHOWN.store(true, Ordering::Relaxed);
            theme::install_fonts(&cc.egui_ctx);
            theme::apply_style(&cc.egui_ctx);
            system.attach(&cc.egui_ctx);
            Ok(make(cc, system))
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

    fn parse(args: &[&str]) -> Result<Command> {
        parse_args(args.iter().map(|arg| arg.to_string()))
    }

    fn error(args: &[&str]) -> String {
        parse(args).expect_err("ожидалась ошибка").to_string()
    }

    #[test]
    fn arguments() {
        let run = |page, theme| Command::Run { page, theme };
        assert_eq!(parse(&[]).unwrap(), run(None, None));
        assert_eq!(
            parse(&["--page", "help", "--theme", "light"]).unwrap(),
            run(Some(app::Page::Help), Some(ThemeMode::Light))
        );
        assert_eq!(
            parse(&["--theme", " dark "]).unwrap(),
            run(None, Some(ThemeMode::Dark))
        );
        for (arg, command) in [
            ("-h", Command::Help),
            ("--help", Command::Help),
            ("-V", Command::Version),
            ("--version", Command::Version),
        ] {
            assert_eq!(parse(&["--page", "log", arg]).unwrap(), command, "{arg}");
        }
    }

    #[test]
    fn mistakes() {
        assert_eq!(error(&["--page", "nope"]), "нет вкладки «nope»");
        assert_eq!(error(&["--page"]), "нет вкладки «»");
        assert_eq!(error(&["--theme", "x"]), "нет темы «x»");
        assert_eq!(error(&["--bogus"]), "неизвестный параметр --bogus");
        assert_eq!(
            error(&["--bogus", "--help"]),
            "неизвестный параметр --bogus"
        );
    }
}
