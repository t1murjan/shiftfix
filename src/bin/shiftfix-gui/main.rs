//! `shiftfix-gui` — графическая оболочка: маленькое окно у значка в трее с
//! вкладками (состояние службы, устройства, настройки, инструкция и журнал)
//! и запуск из меню приложений без окна (`--background`).

mod app;
mod background;
mod edit;
mod format;
mod hyprland;
mod instance;
mod kwin;
mod link;
mod pages;
mod placement;
mod popup;
mod portal;
#[cfg(test)]
mod testutil;
mod theme;
mod toast;
mod update;
mod widgets;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Result, bail};
use eframe::egui;
use shiftfix::config::ThemeMode;
use shiftfix::sys::proc;

const USAGE: &str = "\
shiftfix-gui — настройки и состояние shiftfix

Использование:
  shiftfix-gui [--page home|devices|settings|help|log] [--theme system|light|dark]
  shiftfix-gui --popup [--at X,Y] [--page …] [--theme …]
  shiftfix-gui --background

  --page          открыть вкладку
  --theme         режим оформления только на этот запуск (в настройки не пишется)
  --popup         щелчок по значку в трее: такой же запуск при открытом окне закрывает его
  --at X,Y        где щёлкнули по значку: в X11 окно встанет рядом
  --background    запустить службу со значком в трее, если она не работает, и выйти;
                  окно — только если значка не будет
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
    /// Окно: вкладка, место и режим оформления.
    Open(popup::Open),
    /// Запуск из меню приложений: служба со значком в трее, окно — только
    /// если значка не будет.
    Background,
    Help,
    Version,
}

/// Параметры без имени программы.
fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Command> {
    let mut it = args.into_iter();
    let (mut page, mut theme, mut at) = (None, None, None);
    let (mut popup, mut background) = (false, false);
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
            "--popup" => popup = true,
            "--background" => background = true,
            "--at" => {
                let point = it.next().unwrap_or_default();
                let Some(xy) = parse_point(&point) else {
                    bail!("--at: нужно X,Y, а не «{point}»");
                };
                at = Some(xy);
            }
            "-h" | "--help" => return Ok(Command::Help),
            "-V" | "--version" => return Ok(Command::Version),
            other => bail!("неизвестный параметр {other}"),
        }
    }
    if background {
        if popup || page.is_some() || theme.is_some() || at.is_some() {
            bail!("--background — без других параметров");
        }
        return Ok(Command::Background);
    }
    if at.is_some() && !popup {
        bail!("--at — только с --popup");
    }
    Ok(Command::Open(popup::Open {
        page,
        at,
        toggle: popup,
        theme,
    }))
}

/// «X,Y» — точка щелчка по значку в трее.
fn parse_point(text: &str) -> Option<(i32, i32)> {
    let (x, y) = text.split_once(',')?;
    Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
}

fn main() -> eframe::Result {
    shiftfix::logbuf::init(false);
    match parse_args(std::env::args().skip(1)) {
        Ok(Command::Open(open)) => popup::run(open),
        Ok(Command::Background) => {
            // Значка в трее не будет — без окна до программы не добраться.
            if background::window_needed() {
                popup::run(popup::Open::default())
            } else {
                Ok(())
            }
        }
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
        let run = |page, theme| {
            Command::Open(popup::Open {
                page,
                at: None,
                toggle: false,
                theme,
            })
        };
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
    fn popup_arguments() {
        let popup = |at, theme| {
            Command::Open(popup::Open {
                page: None,
                at,
                toggle: true,
                theme,
            })
        };
        assert_eq!(parse(&["--popup"]).unwrap(), popup(None, None));
        assert_eq!(
            parse(&["--popup", "--at", "1850,1050", "--theme", "dark"]).unwrap(),
            popup(Some((1850, 1050)), Some(ThemeMode::Dark))
        );
        // Порядок любой; на нескольких мониторах координаты бывают отрицательными.
        assert_eq!(
            parse(&["--at", "-1280, 12", "--popup"]).unwrap(),
            popup(Some((-1280, 12)), None)
        );
        assert_eq!(parse(&["--popup", "-V"]).unwrap(), Command::Version);
        // Щелчок по значку может сразу открыть вкладку.
        assert_eq!(
            parse(&["--popup", "--page", "help"]).unwrap(),
            Command::Open(popup::Open {
                page: Some(app::Page::Help),
                at: None,
                toggle: true,
                theme: None,
            })
        );
    }

    #[test]
    fn background_argument() {
        assert_eq!(parse(&["--background"]).unwrap(), Command::Background);
        for other in [
            &["--popup"][..],
            &["--page", "help"],
            &["--theme", "dark"],
            &["--popup", "--at", "1,2"],
        ] {
            let mut args = vec!["--background"];
            args.extend(other);
            assert_eq!(
                error(&args),
                "--background — без других параметров",
                "{other:?}"
            );
        }
    }

    #[test]
    fn mistakes() {
        assert_eq!(error(&["--page", "nope"]), "нет вкладки «nope»");
        assert_eq!(error(&["--page"]), "нет вкладки «»");
        assert_eq!(error(&["--theme", "x"]), "нет темы «x»");
        assert_eq!(error(&["--bogus"]), "неизвестный параметр --bogus");
        assert_eq!(
            error(&["--popup", "--at", "10"]),
            "--at: нужно X,Y, а не «10»"
        );
        assert_eq!(
            error(&["--popup", "--at", "x,1"]),
            "--at: нужно X,Y, а не «x,1»"
        );
        assert_eq!(error(&["--popup", "--at"]), "--at: нужно X,Y, а не «»");
        assert_eq!(error(&["--at", "1,2"]), "--at — только с --popup");
        assert_eq!(
            error(&["--bogus", "--help"]),
            "неизвестный параметр --bogus"
        );
    }
}
