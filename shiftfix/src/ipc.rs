//! Связь GUI (и команд `shiftfix --toggle` и т.п.) со службой: Unix-сокет в
//! XDG_RUNTIME_DIR, один JSON-запрос и один JSON-ответ на соединение.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use log::{debug, warn};
use serde::{Deserialize, Serialize};

use crate::daemon::Msg;
use crate::devices::DeviceInfo;
use crate::engine::{Action, LayoutInfo};
use crate::logbuf::LogLine;

pub fn socket_path() -> PathBuf {
    crate::env::runtime_dir().join("shiftfix.sock")
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

/// Состояние службы для GUI.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Status {
    pub version: String,
    pub pid: u32,
    pub uptime_secs: u64,
    pub enabled: bool,
    pub layouts: LayoutInfo,
    pub selection: bool,
    /// Почему выделение не исправляется (если не исправляется).
    pub selection_note: Option<String>,
    /// Чем сейчас берётся выделение: wl-clipboard или X11.
    #[serde(default)]
    pub selection_via: Option<String>,
    /// Значок показан на панели.
    pub tray: bool,
    /// Значок включён, но панели (StatusNotifierWatcher) пока нет.
    #[serde(default)]
    pub tray_waiting: bool,
    pub devices: Vec<DeviceInfo>,
    pub actions: Vec<Action>,
    pub log: Vec<LogLine>,
    pub config_path: String,
}

/// Отправить запрос службе.
pub fn request(req: &Request, timeout: Duration) -> Result<Response> {
    let path = socket_path();
    let mut stream = UnixStream::connect(&path)
        .with_context(|| format!("служба не отвечает ({})", path.display()))?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    let mut line = serde_json::to_string(req)?;
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    let mut answer = String::new();
    BufReader::new(stream)
        .read_line(&mut answer)
        .context("служба не ответила")?;
    let response: Response = serde_json::from_str(&answer).context("непонятный ответ службы")?;
    if !response.ok {
        bail!(
            "{}",
            response.error.unwrap_or_else(|| "ошибка службы".into())
        );
    }
    Ok(response)
}

/// Слушать сокет; каждый запрос уходит в главный цикл как `Msg::Ipc`.
pub fn serve(tx: Sender<Msg>) -> Result<()> {
    let path = socket_path();
    let _ = std::fs::remove_file(&path); // второй экземпляр уже исключён блокировкой
    let listener = UnixListener::bind(&path)
        .with_context(|| format!("не создаётся сокет {}", path.display()))?;
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    thread::Builder::new().name("ipc".into()).spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let tx = tx.clone();
                    let _ = thread::Builder::new()
                        .name("ipc client".into())
                        .spawn(move || handle(stream, &tx));
                }
                Err(err) => warn!("IPC: {err}"),
            }
        }
    })?;
    Ok(())
}

fn handle(stream: UnixStream, tx: &Sender<Msg>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let mut line = String::new();
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    if BufReader::new(stream).read_line(&mut line).is_err() {
        return;
    }
    let response = match serde_json::from_str::<Request>(&line) {
        Ok(req) => {
            debug!("IPC: {req:?}");
            let (reply, answer) = mpsc::channel();
            if tx.send(Msg::Ipc(req, reply)).is_err() {
                return;
            }
            // Пока служба печатает исправление, ответ подождёт.
            answer
                .recv_timeout(Duration::from_secs(5))
                .unwrap_or_else(|_| Response::error("служба занята"))
        }
        Err(err) => Response::error(format!("непонятный запрос: {err}")),
    };
    if let Ok(mut text) = serde_json::to_string(&response) {
        text.push('\n');
        let _ = writer.write_all(text.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
