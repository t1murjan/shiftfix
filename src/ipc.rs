//! Связь GUI (и команд `shiftfix --toggle` и т.п.) со службой: Unix-сокет в
//! XDG_RUNTIME_DIR, один JSON-запрос и один JSON-ответ на соединение. Здесь —
//! запросы, ответы и клиент; сервер — в `daemon/socket.rs`.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::devices::DeviceInfo;
use crate::engine::{Action, LayoutInfo};
use crate::logbuf::LogLine;

/// Сколько клиент — команды `shiftfix` и окно — ждёт ответа службы.
pub const TIMEOUT: Duration = Duration::from_secs(3);
/// Сколько запрос может ждать очереди в главном цикле службы: тот бывает
/// занят — печатает исправление. Опоздавший запрос не выполняется: сервер
/// скоро перестанет ждать ответа, и выполненный запрос клиент увидел бы как
/// «служба занята». На сам запрос остаётся REPLY_WAIT − QUEUE_LIMIT.
pub(crate) const QUEUE_LIMIT: Duration = Duration::from_millis(1500);
/// Сколько сервер ждёт ответа главного цикла: очередь и сам запрос. Меньше
/// TIMEOUT, чтобы «служба занята» успела дойти до клиента.
pub(crate) const REPLY_WAIT: Duration = Duration::from_millis(2500);
const _: () = assert!(
    QUEUE_LIMIT.as_millis() < REPLY_WAIT.as_millis()
        && REPLY_WAIT.as_millis() < TIMEOUT.as_millis()
);
/// Самая длинная строка запроса; длиннее — «непонятный запрос».
pub(crate) const MAX_REQUEST: u64 = 64 * 1024;

pub(crate) fn socket_path() -> PathBuf {
    crate::sys::paths::runtime_dir().join("shiftfix.sock")
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Status,
    SetEnabled {
        enabled: bool,
    },
    Toggle,
    /// Перечитать конфиг.
    Reload,
    Quit,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<Box<Status>>,
}

impl Response {
    pub fn ok() -> Response {
        Response {
            ok: true,
            ..Response::default()
        }
    }

    pub fn error(text: impl Into<String>) -> Response {
        Response {
            ok: false,
            error: Some(text.into()),
            status: None,
        }
    }
}

/// Состояние службы для GUI. Окно и служба обновляются вместе, но между
/// установкой и перезапуском службы окно бывает новее: полей, которых
/// служба ещё не знает, в ответе нет — они берутся по умолчанию. Так же
/// в записях `devices` и `log`, в `layouts` и `actions`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Status {
    pub version: String,
    pub pid: u32,
    pub enabled: bool,
    pub layouts: LayoutInfo,
    pub selection: bool,
    /// Почему выделение не исправляется (если не исправляется).
    pub selection_note: Option<String>,
    /// Чем сейчас берётся выделение: wl-clipboard или X11.
    pub selection_via: Option<String>,
    /// Значок показан на панели.
    pub tray: bool,
    /// Значок включён, но панели (StatusNotifierWatcher) пока нет.
    pub tray_waiting: bool,
    pub devices: Vec<DeviceInfo>,
    pub actions: Vec<Action>,
    pub log: Vec<LogLine>,
    pub config_path: String,
}

/// Отправить запрос службе и подождать ответа не дольше TIMEOUT.
pub fn request(req: &Request) -> Result<Response> {
    request_at(&socket_path(), req)
}

pub(crate) fn request_at(path: &Path, req: &Request) -> Result<Response> {
    let mut stream = UnixStream::connect(path)
        .with_context(|| format!("служба не отвечает ({})", path.display()))?;
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    let mut line = serde_json::to_string(req)?;
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    let mut answer = String::new();
    BufReader::new(stream)
        .read_line(&mut answer)
        .context("служба не ответила")?;
    if answer.is_empty() {
        bail!("служба закрыла соединение");
    }
    let response: Response = serde_json::from_str(&answer).context("непонятный ответ службы")?;
    if !response.ok {
        bail!(
            "{}",
            response.error.unwrap_or_else(|| "ошибка службы".into())
        );
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;
    use std::thread;

    use super::*;
    use crate::devices::Role;
    use crate::engine::ActionKind;
    use crate::testutil::TempDir;

    #[test]
    fn requests_are_tagged_json() {
        assert_eq!(
            serde_json::to_string(&Request::Toggle).unwrap(),
            r#"{"cmd":"toggle"}"#
        );
        let req: Request =
            serde_json::from_str(r#"{"cmd":"set_enabled","enabled":false}"#).unwrap();
        assert_eq!(req, Request::SetEnabled { enabled: false });
        let resp: Response = serde_json::from_str(r#"{"ok":true}"#).unwrap();
        assert!(resp.ok && resp.status.is_none());
    }

    #[test]
    fn status_from_other_version() {
        // Служба старее окна: чего нет в ответе — по умолчанию.
        let resp: Response =
            serde_json::from_str(r#"{"ok":true,"status":{"version":"0.3.0","enabled":true}}"#)
                .unwrap();
        let status = resp.status.unwrap();
        assert_eq!((status.version.as_str(), status.enabled), ("0.3.0", true));
        assert_eq!(status.pid, 0);
        assert!(status.devices.is_empty() && status.selection_via.is_none());
        assert!(status.layouts.codes.is_empty() && !status.tray_waiting);
        // То же в записях устройств и журнала.
        let status: Status = serde_json::from_str(
            r#"{"devices":[{"name":"kbd","role":"keyboard"}],"log":[{"text":"готов"}]}"#,
        )
        .unwrap();
        let device = &status.devices[0];
        assert_eq!((device.name.as_str(), device.role), ("kbd", Role::Keyboard));
        assert_eq!((device.detected, device.idle_ms), (Role::Other, None));
        assert_eq!(
            (status.log[0].text.as_str(), status.log[0].time),
            ("готов", 0)
        );
        // И в раскладках и исправлениях.
        let status: Status = serde_json::from_str(
            r#"{"layouts":{"backend":"KDE"},"actions":[{"kind":"undo","chars":3}]}"#,
        )
        .unwrap();
        let layouts = &status.layouts;
        assert_eq!((layouts.backend.as_str(), layouts.current), ("KDE", None));
        assert!(layouts.codes.is_empty() && !layouts.from_keymap && layouts.error.is_none());
        let action = &status.actions[0];
        assert_eq!(
            (action.kind, action.chars, action.time),
            (ActionKind::Undo, 3, 0)
        );
        assert!(action.from.is_empty() && action.to.is_empty());
        // Незнакомые поля пропускаются: служба новее окна или прежняя,
        // с uptime_secs.
        let status: Status =
            serde_json::from_str(r#"{"version":"9.0.0","uptime_secs":5,"future":[1]}"#).unwrap();
        assert_eq!(status.version, "9.0.0");
    }

    #[test]
    fn install_stand_status_matches() {
        // Поддельная служба стенда tests/install отвечает на status этим
        // файлом. Поле, которого нет в Status или которое Status не
        // вернёт, значит, что подделка разошлась со службой.
        let fake: serde_json::Value =
            serde_json::from_str(include_str!("../tests/install/fake_status.json")).unwrap();
        let status: Status = serde_json::from_value(fake.clone()).unwrap();
        assert_eq!(serde_json::to_value(&status).unwrap(), fake);
    }

    #[test]
    fn closed_connection_is_reported() {
        let dir = TempDir::new("ipc");
        let path = dir.join("shiftfix.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(stream).read_line(&mut line).unwrap();
        });
        let err = request_at(&path, &Request::Status).unwrap_err();
        assert_eq!(err.to_string(), "служба закрыла соединение");
        server.join().unwrap();
    }
}
