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

pub fn socket_path() -> PathBuf {
    crate::sys::env::runtime_dir().join("shiftfix.sock")
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
    request_at(&socket_path(), req, timeout)
}

pub(crate) fn request_at(path: &Path, req: &Request, timeout: Duration) -> Result<Response> {
    let mut stream = UnixStream::connect(path)
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
