//! Сервер IPC: слушает `ipc::socket_path()`, на каждое соединение — поток,
//! запрос уходит в главный цикл службы. Протокол и клиент — в `ipc.rs`.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use log::{debug, warn};

use super::Msg;
use crate::ipc::{Request, Response};

/// Сторож сокета: удаляет файл в Drop.
pub struct Socket(PathBuf);

impl Drop for Socket {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Слушать сокет `path`; каждый запрос уходит в главный цикл как `Msg::Ipc`.
/// Файл сокета живёт, пока жив возвращённый сторож.
pub fn serve(path: PathBuf, tx: Sender<Msg>) -> Result<Socket> {
    let _ = fs::remove_file(&path); // второй экземпляр уже исключён блокировкой
    let listener = UnixListener::bind(&path)
        .with_context(|| format!("не создаётся сокет {}", path.display()))?;
    let socket = Socket(path);
    let _ = fs::set_permissions(&socket.0, fs::Permissions::from_mode(0o600));
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
    Ok(socket)
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
    use std::path::Path;

    use super::*;
    use crate::ipc::{Status, request_at};
    use crate::testutil::TempDir;

    /// Сервер на сокете во временном каталоге и главный цикл, который
    /// отвечает на status версией «тест», на остальное — ok.
    fn server(dir: &TempDir) -> Socket {
        let (tx, rx) = mpsc::channel();
        let socket = serve(dir.join("shiftfix.sock"), tx).unwrap();
        thread::spawn(move || {
            for msg in rx {
                if let Msg::Ipc(req, reply) = msg {
                    let status = (req == Request::Status).then(|| {
                        Box::new(Status {
                            version: "тест".into(),
                            ..Status::default()
                        })
                    });
                    let _ = reply.send(Response {
                        status,
                        ..Response::ok()
                    });
                }
            }
        });
        socket
    }

    /// Послать строку как есть и прочитать ответ.
    fn raw(path: &Path, line: &str) -> Response {
        let mut stream = UnixStream::connect(path).unwrap();
        stream.write_all(line.as_bytes()).unwrap();
        let mut answer = String::new();
        BufReader::new(stream).read_line(&mut answer).unwrap();
        serde_json::from_str(&answer).unwrap()
    }

    #[test]
    fn socket_answers_requests() {
        let dir = TempDir::new("ipc");
        let socket = server(&dir);
        let path = socket.0.clone();
        let timeout = Duration::from_secs(3);

        let resp = request_at(&path, &Request::Status, timeout).unwrap();
        assert_eq!(resp.status.unwrap().version, "тест");
        assert!(request_at(&path, &Request::Toggle, timeout).unwrap().ok);

        for line in ["{\"cmd\":\"fly\"}\n", "не json\n", "\n"] {
            let resp = raw(&path, line);
            assert!(!resp.ok, "{line}");
            assert!(
                resp.error.unwrap().starts_with("непонятный запрос"),
                "{line}"
            );
        }

        drop(socket);
        assert!(!path.exists(), "сторож удаляет файл сокета");
    }
}
