//! `shiftfix` — служба: двойной Shift исправляет раскладку последнего слова
//! или выделенного текста. Без параметров запускает службу; параметры
//! `--toggle`, `--status` и т.п. управляют уже запущенной.

use std::fmt::Display;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use log::error;
use shiftfix::config::{self, Config};
use shiftfix::devices::{self, Role};
use shiftfix::ipc::{self, Request};
use shiftfix::{backend, env, logbuf, proc, xkb};

const USAGE: &str = "\
shiftfix — двойной Shift исправляет раскладку последнего слова или выделенного текста

Использование: shiftfix [ПАРАМЕТРЫ]

Без параметров запускает службу (обычно это делает systemd).

  --status          состояние запущенной службы
  --toggle          пауза / продолжить (удобно повесить на горячую клавишу)
  --pause, --resume поставить на паузу / снять с паузы
  --reload          перечитать конфиг
  --quit            остановить службу
  --check           проверить окружение и выйти
  --print-config    напечатать конфиг по умолчанию
  --config ПУТЬ     конфиг (по умолчанию ~/.config/shiftfix/config.toml)
  --debug           подробный лог (набранный текст в лог не попадает)
  -V, --version     версия
  -h, --help        эта справка

Настройки, устройства и инструкция — в программе shiftfix-gui.
";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Command {
    Run,
    Check,
    PrintConfig,
    Help,
    Version,
    Status,
    Send(&'static str),
}

struct Args {
    command: Command,
    config: Option<PathBuf>,
    debug: bool,
}

fn parse_args() -> Result<Args> {
    let mut args = Args {
        command: Command::Run,
        config: None,
        debug: false,
    };
    let mut it = std::env::args_os().skip(1);
    while let Some(arg) = it.next() {
        let command = match arg.to_str() {
            Some("--config") => {
                args.config = Some(it.next().context("--config: нужен путь")?.into());
                continue;
            }
            Some("--debug") => {
                args.debug = true;
                continue;
            }
            Some("--check") => Command::Check,
            Some("--print-config") => Command::PrintConfig,
            Some("-h" | "--help") => Command::Help,
            Some("-V" | "--version") => Command::Version,
            Some("--status") => Command::Status,
            Some("--toggle") => Command::Send("toggle"),
            Some("--pause") => Command::Send("pause"),
            Some("--resume") => Command::Send("resume"),
            Some("--reload") => Command::Send("reload"),
            Some("--quit") => Command::Send("quit"),
            _ => bail!("неизвестный параметр {}", arg.to_string_lossy()),
        };
        if args.command != Command::Run {
            bail!("можно указать только одно действие");
        }
        args.command = command;
    }
    Ok(args)
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(err) => {
            eprintln!("shiftfix: {err}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match args.command {
        Command::Help => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Command::Version => {
            println!("shiftfix {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Command::PrintConfig => {
            print!("{}", config::DEFAULT_CONFIG);
            return ExitCode::SUCCESS;
        }
        Command::Status => return status(),
        Command::Send(what) => return send(what),
        Command::Run | Command::Check => {}
    }
    logbuf::init(args.debug);
    let path = args.config.unwrap_or_else(Config::default_path);
    let cfg = match Config::load(&path) {
        Ok(cfg) => cfg,
        Err(err) => {
            error!("config: {err:#}");
            return ExitCode::from(2);
        }
    };
    if args.command == Command::Check {
        return check(&cfg);
    }
    match shiftfix::daemon::run(path, cfg) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            error!("{err:#}");
            ExitCode::from(2) // systemd не перезапускает (RestartPreventExitStatus=2)
        }
    }
}

// ─── Управление запущенной службой ──────────────────────────────────────────

fn send(what: &str) -> ExitCode {
    let req = match what {
        "toggle" => Request::Toggle,
        "pause" => Request::SetEnabled { enabled: false },
        "resume" => Request::SetEnabled { enabled: true },
        "reload" => Request::Reload,
        _ => Request::Quit,
    };
    match ipc::request(&req, Duration::from_secs(3)) {
        Ok(_) if what == "toggle" => status(),
        Ok(_) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("shiftfix: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn status() -> ExitCode {
    match ipc::request(&Request::Status, Duration::from_secs(3)) {
        Ok(response) => {
            let Some(s) = response.status else {
                return ExitCode::FAILURE;
            };
            println!(
                "shiftfix {} (pid {}): {}",
                s.version,
                s.pid,
                if s.enabled {
                    "работает"
                } else {
                    "на паузе"
                }
            );
            println!(
                "раскладки: {} — {}",
                s.layouts.codes.join(", "),
                s.layouts.backend
            );
            let keyboards = s
                .devices
                .iter()
                .filter(|d| d.role == Role::Keyboard)
                .count();
            println!(
                "клавиатур: {keyboards}, устройств всего: {}",
                s.devices.len()
            );
            match s.selection_note {
                None => println!("выделение: исправляется"),
                Some(note) => println!("выделение: не исправляется ({note})"),
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("shiftfix: {err:#}");
            ExitCode::FAILURE
        }
    }
}

// ─── Проверка окружения ─────────────────────────────────────────────────────

fn ok(msg: impl Display) {
    println!("  ✓ {msg}");
}

fn bad(problems: &mut usize, msg: impl Display) {
    *problems += 1;
    println!("  ✗ {msg}");
}

fn check(cfg: &Config) -> ExitCode {
    let mut problems = 0;

    println!("Устройства ввода");
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
        ok(format!("клавиатур: {keyboards}"));
    } else if unreadable > 0 {
        bad(
            &mut problems,
            "нет доступа к /dev/input — sudo usermod -aG input $USER, затем перезагрузиться",
        );
    } else {
        bad(
            &mut problems,
            "клавиатуры не найдены — назначь роль вручную в shiftfix-gui, вкладка «Устройства»",
        );
    }

    println!("Виртуальная клавиатура");
    if fs::OpenOptions::new()
        .write(true)
        .open("/dev/uinput")
        .is_ok()
    {
        ok("/dev/uinput доступен на запись");
    } else {
        bad(
            &mut problems,
            "/dev/uinput недоступен — нужно udev-правило из install.sh",
        );
    }

    println!("Раскладки");
    let mut switcher = backend::make(cfg);
    match switcher.layouts() {
        Ok(codes) => {
            let current = switcher
                .current()
                .ok()
                .flatten()
                .and_then(|i| codes.get(i))
                .map_or("неизвестна", String::as_str);
            ok(format!(
                "переключает {}: {}; сейчас {current}",
                switcher.name(),
                codes.join(", ")
            ));
            if codes.len() < 2 {
                bad(&mut problems, "нужно минимум две раскладки");
            }
        }
        Err(err) => bad(&mut problems, format!("{}: {err:#}", switcher.name())),
    }
    match xkb::fetch() {
        Some(keymap) => {
            let names: Vec<&str> = keymap.layouts.iter().map(|l| l.name.as_str()).collect();
            ok(format!("keymap композитора: {}", names.join(", ")));
        }
        None => println!(
            "  · keymap композитора недоступна — для выделения будут встроенные таблицы us/ru"
        ),
    }

    println!("Выделение");
    if !cfg.selection {
        println!("  · выключено в конфиге");
    } else if !(env::in_path("wl-paste") && env::in_path("wl-copy")) {
        bad(&mut problems, "нет wl-clipboard (пакет wl-clipboard)");
    } else if env::wayland_display().is_none() {
        bad(
            &mut problems,
            "не найден сокет Wayland — это не Wayland-сессия?",
        );
    } else {
        match proc::run("wl-paste", &["--primary", "--list-types"], proc::TIMEOUT) {
            Ok(out) => {
                let reason = out.stderr.to_lowercase();
                if out.ok || reason.contains("nothing") || reason.contains("no selection") {
                    ok("PRIMARY читается");
                } else {
                    bad(&mut problems, format!("PRIMARY недоступен: {}", out.stderr));
                }
            }
            Err(err) => bad(&mut problems, format!("{err:#}")),
        }
    }

    if problems == 0 {
        println!("\nВсё готово.");
        ExitCode::SUCCESS
    } else {
        println!("\nПроблем: {problems}.");
        ExitCode::FAILURE
    }
}
