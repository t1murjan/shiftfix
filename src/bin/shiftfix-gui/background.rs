//! Запуск из меню приложений (`shiftfix-gui --background`): служба со значком
//! в трее, без окна. Служба не работает — запустить её так же, как кнопка
//! «Запустить» (systemd или процессом), и выйти: окно открывают через значок.
//! Окно открывается сразу, только если значка не будет, — иначе до программы
//! не добраться.

use std::thread;
use std::time::{Duration, Instant};

use shiftfix::config::{self, Settings};
use shiftfix::ipc::{self, Request, Status};

use crate::link::{self, ServiceAction};

/// Сколько ждать ответа только что запущенной службы: она ищет устройства, а
/// если они закрыты правами — ждёт секунду и ищет ещё раз.
const START_WAIT: Duration = Duration::from_secs(5);

/// Запустить службу, если она не работает, и сказать, нужно ли окно.
pub fn window_needed() -> bool {
    // Конфиг с ошибкой — как без значка: служба с ним не запустится, а окно
    // покажет, что не так.
    let tray_in_config = Settings::load(&config::default_path()).is_ok_and(|s| s.tray);
    let status = if tray_in_config {
        running_service()
    } else {
        None
    };
    needs_window(tray_in_config, status.as_ref())
}

/// Окно нужно, если значка в трее не будет: он выключен в конфиге, служба не
/// ответила или сообщает, что значка на панели нет (панели с треем нет или
/// значок не создался).
fn needs_window(tray_in_config: bool, status: Option<&Status>) -> bool {
    !tray_in_config || !status.is_some_and(|s| s.tray)
}

/// Состояние службы; не работает — запустить её и дождаться ответа.
fn running_service() -> Option<Status> {
    if let Some(status) = status() {
        return Some(status);
    }
    if let Err(err) = link::run_service_action(ServiceAction::Start) {
        log::warn!("служба не запустилась: {err:#}");
        return None;
    }
    let deadline = Instant::now() + START_WAIT;
    while Instant::now() < deadline {
        thread::sleep(Duration::from_millis(200));
        if let Some(status) = status() {
            return Some(status);
        }
    }
    log::warn!(
        "служба не ответила за {} с после запуска",
        START_WAIT.as_secs()
    );
    None
}

fn status() -> Option<Status> {
    let response = ipc::request(&Request::Status).ok()?;
    response.status.map(|status| *status)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_only_without_tray_icon() {
        let status = |tray, tray_waiting| Status {
            tray,
            tray_waiting,
            ..Status::default()
        };
        // Значок на панели — окно не нужно.
        assert!(!needs_window(true, Some(&status(true, false))));
        // Значок выключен в конфиге — окно, что бы ни сказала служба.
        assert!(needs_window(false, None));
        assert!(needs_window(false, Some(&status(true, false))));
        // Панели с треем нет, значок не создался, служба не ответила.
        assert!(needs_window(true, Some(&status(false, true))));
        assert!(needs_window(true, Some(&status(false, false))));
        assert!(needs_window(true, None));
    }
}
