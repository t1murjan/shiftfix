//! `shiftfix` — служба: двойной Shift исправляет раскладку последнего слова
//! или выделенного текста. Без параметров запускает службу; параметры
//! `--toggle`, `--status` и т.п. управляют уже запущенной.

mod check;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use log::error;
use shiftfix::config::{self, Config};
use shiftfix::devices::Role;
use shiftfix::ipc::{self, Request};
use shiftfix::{logbuf, service};

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
  --print-unit      напечатать юнит systemd user для этого бинарника
  --print-autostart напечатать файл автозапуска (.desktop) — для сеанса без systemd
  --config ПУТЬ     конфиг (по умолчанию ~/.config/shiftfix/config.toml)
  --debug           подробный лог (набранный текст в лог не попадает)
  -V, --version     версия
  -h, --help        эта справка

Настройки, устройства и инструкция — в программе shiftfix-gui.
";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Run,
    Check,
    PrintConfig,
    PrintUnit,
    PrintAutostart,
    Help,
    Version,
    Status,
    /// Команда запущенной службе.
    Send(Request),
}

struct Args {
    command: Command,
    config: Option<PathBuf>,
    debug: bool,
}

/// Параметры без имени программы.
fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<Args> {
    let mut it = args.into_iter();
    let mut args = Args {
        command: Command::Run,
        config: None,
        debug: false,
    };
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
            Some("--print-unit") => Command::PrintUnit,
            Some("--print-autostart") => Command::PrintAutostart,
            Some("-h" | "--help") => Command::Help,
            Some("-V" | "--version") => Command::Version,
            Some("--status") => Command::Status,
            Some("--toggle") => Command::Send(Request::Toggle),
            Some("--pause") => Command::Send(Request::SetEnabled { enabled: false }),
            Some("--resume") => Command::Send(Request::SetEnabled { enabled: true }),
            Some("--reload") => Command::Send(Request::Reload),
            Some("--quit") => Command::Send(Request::Quit),
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
    let args = match parse_args(std::env::args_os().skip(1)) {
        Ok(args) => args,
        Err(err) => {
            eprintln!("shiftfix: {err}\n\n{USAGE}");
            return ExitCode::from(service::EXIT_SETUP);
        }
    };
    match &args.command {
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
        Command::PrintUnit => {
            return print_for_exe(|exe| {
                service::unit_text(exe, service::graphical_target_active())
            });
        }
        Command::PrintAutostart => return print_for_exe(service::autostart_text),
        Command::Status => return status(),
        Command::Send(req) => return send(req),
        Command::Run | Command::Check => {}
    }
    logbuf::init(args.debug);
    let path = args.config.unwrap_or_else(Config::default_path);
    let cfg = match Config::load(&path) {
        Ok(cfg) => cfg,
        Err(err) => {
            error!("config: {err:#}");
            return ExitCode::from(service::EXIT_SETUP);
        }
    };
    if args.command == Command::Check {
        return check::run(&cfg);
    }
    match shiftfix::daemon::run(path, cfg) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            error!("{err:#}");
            ExitCode::from(service::EXIT_SETUP) // systemd не перезапускает
        }
    }
}

/// Напечатать текст для этого бинарника: юнит systemd или файл автозапуска.
fn print_for_exe(text: impl FnOnce(&Path) -> String) -> ExitCode {
    match std::env::current_exe() {
        Ok(exe) => {
            print!("{}", text(&exe));
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("shiftfix: не найден путь к бинарнику: {err}");
            ExitCode::FAILURE
        }
    }
}

fn send(req: &Request) -> ExitCode {
    match ipc::request(req, Duration::from_secs(3)) {
        // После переключения видно, в какую сторону.
        Ok(_) if *req == Request::Toggle => status(),
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
            match (s.selection_note, s.selection_via) {
                (None, Some(via)) => println!("выделение: исправляется (через {via})"),
                (None, None) => println!("выделение: исправляется"),
                (Some(note), _) => println!("выделение: не исправляется ({note})"),
            }
            if s.tray_waiting {
                println!("трей: жду появления панели");
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("shiftfix: {err:#}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::ffi::OsStringExt;

    use super::*;

    fn parse(args: &[&str]) -> Result<Args> {
        parse_args(args.iter().map(OsString::from))
    }

    fn error(args: &[&str]) -> String {
        parse(args).err().expect("ожидалась ошибка").to_string()
    }

    #[test]
    fn without_arguments_runs_service() {
        let args = parse(&[]).unwrap();
        assert_eq!(args.command, Command::Run);
        assert!(args.config.is_none() && !args.debug);
    }

    #[test]
    fn options_and_actions() {
        let args = parse(&["--debug", "--config", "/tmp/my config.toml", "--check"]).unwrap();
        assert_eq!(args.command, Command::Check);
        assert_eq!(args.config, Some(PathBuf::from("/tmp/my config.toml")));
        assert!(args.debug);

        for (arg, command) in [
            ("--check", Command::Check),
            ("--print-config", Command::PrintConfig),
            ("--print-unit", Command::PrintUnit),
            ("--print-autostart", Command::PrintAutostart),
            ("-h", Command::Help),
            ("--help", Command::Help),
            ("-V", Command::Version),
            ("--version", Command::Version),
            ("--status", Command::Status),
            ("--toggle", Command::Send(Request::Toggle)),
            (
                "--pause",
                Command::Send(Request::SetEnabled { enabled: false }),
            ),
            (
                "--resume",
                Command::Send(Request::SetEnabled { enabled: true }),
            ),
            ("--reload", Command::Send(Request::Reload)),
            ("--quit", Command::Send(Request::Quit)),
        ] {
            assert_eq!(parse(&[arg]).unwrap().command, command, "{arg}");
        }

        // Путь к конфигу — любые байты, не только UTF-8.
        let path = OsString::from_vec(b"/tmp/\xff.toml".to_vec());
        let args = parse_args([OsString::from("--config"), path.clone()]).unwrap();
        assert_eq!(args.config, Some(PathBuf::from(path)));
    }

    #[test]
    fn mistakes() {
        assert_eq!(
            error(&["--status", "--toggle"]),
            "можно указать только одно действие"
        );
        assert_eq!(
            error(&["--pause", "--pause"]),
            "можно указать только одно действие"
        );
        assert_eq!(error(&["--config"]), "--config: нужен путь");
        assert_eq!(error(&["--bogus"]), "неизвестный параметр --bogus");
        assert_eq!(error(&["status"]), "неизвестный параметр status");
        let bad = parse_args([OsString::from_vec(b"--\xff".to_vec())]);
        assert_eq!(
            bad.err().unwrap().to_string(),
            "неизвестный параметр --\u{fffd}"
        );
    }
}
