//! `shiftfix --check`: проверить окружение и сказать, чего не хватает.

use std::fmt::Display;
use std::fs;
use std::process::ExitCode;

use shiftfix::config::Config;
use shiftfix::devices::{self, Role};
use shiftfix::selection::{self, Choice};
use shiftfix::sys::session::Logind;
use shiftfix::sys::x11;
use shiftfix::sys::{env, proc};
use shiftfix::{backend, keymap};

/// Строки отчёта и счётчик проблем.
#[derive(Default)]
struct Report {
    problems: usize,
}

impl Report {
    fn section(&self, title: &str) {
        println!("{title}");
    }

    fn ok(&self, msg: impl Display) {
        println!("  ✓ {msg}");
    }

    fn bad(&mut self, msg: impl Display) {
        self.problems += 1;
        println!("  ✗ {msg}");
    }

    fn note(&self, msg: impl Display) {
        println!("  · {msg}");
    }
}

pub fn run(cfg: &Config) -> ExitCode {
    let mut report = Report::default();
    input_devices(&mut report, cfg);
    uinput(&mut report);
    layouts(&mut report, cfg);
    session(&mut report);
    selection(&mut report, cfg);

    if report.problems == 0 {
        println!("\nВсё готово.");
        ExitCode::SUCCESS
    } else {
        println!("\nПроблем: {}.", report.problems);
        ExitCode::FAILURE
    }
}

fn input_devices(report: &mut Report, cfg: &Config) {
    report.section("Устройства ввода");
    let mut keyboards = 0;
    let mut unreadable = 0;
    for path in devices::event_nodes() {
        let Some(dev) = devices::describe(&path, cfg) else {
            unreadable += 1;
            continue;
        };
        if dev.detected == Role::Other && !dev.overridden {
            continue;
        }
        let manual = if dev.overridden {
            ", задано вручную"
        } else {
            ""
        };
        let virt = if dev.virtual_device {
            ", виртуальное"
        } else {
            ""
        };
        println!(
            "    {}: {} — {}{manual}{virt}",
            dev.path,
            dev.name,
            dev.role.title()
        );
        keyboards += usize::from(dev.role == Role::Keyboard);
    }
    if keyboards > 0 {
        report.ok(format!("клавиатур: {keyboards}"));
    } else if unreadable > 0 {
        report
            .bad("нет доступа к /dev/input — sudo usermod -aG input $USER, затем перезагрузиться");
    } else {
        report.bad(
            "клавиатуры не найдены — назначь роль вручную в shiftfix-gui, вкладка «Устройства»",
        );
    }
}

fn uinput(report: &mut Report) {
    report.section("Виртуальная клавиатура");
    if fs::OpenOptions::new()
        .write(true)
        .open("/dev/uinput")
        .is_ok()
    {
        report.ok("/dev/uinput доступен на запись");
    } else {
        report.bad("/dev/uinput недоступен — нужно udev-правило из install.sh");
    }
}

fn layouts(report: &mut Report, cfg: &Config) {
    report.section("Раскладки");
    let mut switcher = backend::make(cfg);
    match switcher.layouts() {
        Ok(codes) => {
            let current = switcher
                .current()
                .ok()
                .flatten()
                .and_then(|i| codes.get(i))
                .map_or("неизвестна", String::as_str);
            report.ok(format!(
                "переключает {}: {}; сейчас {current}",
                switcher.name(),
                codes.join(", ")
            ));
            if codes.len() < 2 {
                report.bad("нужно минимум две раскладки");
            }
        }
        Err(err) => report.bad(format!("{}: {err:#}", switcher.name())),
    }
    match keymap::wayland::fetch() {
        Some(keymap) => report.ok(format!("keymap композитора: {}", keymap.names())),
        None => match x11_keymap() {
            Some(names) => report.ok(format!("keymap X11: {names}")),
            None => report.note("keymap недоступна — для выделения будут встроенные таблицы us/ru"),
        },
    }
}

/// В X11 keymap не присылает композитор: служба строит её из `_XKB_RULES_NAMES`.
fn x11_keymap() -> Option<String> {
    if env::wayland_display().is_some() {
        return None;
    }
    Some(keymap::x11::fetch(&x11::connect().ok()?)?.names())
}

fn session(report: &mut Report) {
    report.section("Сеанс");
    if env::graphical_session() {
        report.ok("графический сеанс найден");
    } else {
        report.bad("графический сеанс не найден");
    }
    match Logind::default().active() {
        Some(true) => report.ok("сеанс активен (logind)"),
        Some(false) => report.bad(
            "сеанс неактивен (logind): работает другой пользователь или открыта консоль — \
             исправления не выполняются",
        ),
        None => report.note("logind недоступен — активность сеанса не проверяется"),
    }
}

fn selection(report: &mut Report, cfg: &Config) {
    report.section("Выделение");
    let xclip = if env::in_path("xclip") {
        "xclip есть"
    } else {
        "xclip нет"
    };
    let wl_clipboard = if env::in_path("wl-paste") && env::in_path("wl-copy") {
        "wl-clipboard есть"
    } else {
        "wl-clipboard нет"
    };
    report.note(format!("{wl_clipboard}, {xclip}"));
    if !cfg.selection {
        report.note("выключено в конфиге");
        return;
    }
    match selection::decide() {
        Choice::Wait(reason, _) => report.bad(reason),
        Choice::X11 => report.ok("PRIMARY берётся через X11 (XFixes + xclip)"),
        Choice::Wayland => {
            match proc::run("wl-paste", &["--primary", "--list-types"], proc::TIMEOUT) {
                Ok(out) => {
                    let reason = out.stderr.to_lowercase();
                    if out.ok || reason.contains("nothing") || reason.contains("no selection") {
                        report.ok("PRIMARY читается через wl-clipboard");
                    } else {
                        report.bad(format!("PRIMARY недоступен: {}", out.stderr));
                    }
                }
                Err(err) => report.bad(format!("{err:#}")),
            }
        }
    }
}
