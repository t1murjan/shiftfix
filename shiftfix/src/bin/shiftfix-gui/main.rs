//! `shiftfix-gui` — графическая оболочка: состояние службы, устройства,
//! настройки, инструкция и журнал.

mod app;
mod link;
mod pages;
mod theme;
mod widgets;

use std::sync::Arc;

use eframe::egui;

const USAGE: &str = "\
shiftfix-gui — настройки и состояние shiftfix

Использование: shiftfix-gui [--page home|devices|settings|help|log] [--theme system|light|dark]
";

fn main() -> eframe::Result {
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
            "--theme" => match args.next().as_deref().and_then(theme::ThemeChoice::parse) {
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
            other => {
                eprintln!("shiftfix-gui: неизвестный параметр {other}\n\n{USAGE}");
                std::process::exit(2);
            }
        }
    }

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
    eframe::run_native(
        "shiftfix",
        options,
        Box::new(move |cc| {
            theme::install_fonts(&cc.egui_ctx);
            theme::apply(&cc.egui_ctx);
            let choice = forced_theme.unwrap_or_else(theme::saved_choice);
            theme::set_choice(&cc.egui_ctx, choice, false);
            Ok(Box::new(app::App::new(cc, page, choice)))
        }),
    )
}
