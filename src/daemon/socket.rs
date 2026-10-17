//! Сервер IPC: слушает `ipc::socket_path()`, на каждое соединение — поток,
//! запрос уходит в главный цикл службы. Протокол и клиент — в `ipc.rs`.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use log::{debug, warn};

use super::Msg;
use crate::ipc::{self, Request, Response};

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
                Err(err) => {
                    warn!("IPC: {err}");
                    // Ошибка бывает постоянной (кончились дескрипторы) —
                    // не крутиться вхолостую.
                    thread::sleep(Duration::from_millis(200));
                }
            }
        }
    })?;
    Ok(socket)
}

fn handle(stream: UnixStream, tx: &Sender<Msg>) {
    let _ = stream.set_read_timeout(Some(ipc::TIMEOUT));
    let _ = stream.set_write_timeout(Some(ipc::TIMEOUT));
    if let Ok(writer) = stream.try_clone() {
        respond(stream, writer, tx);
    }
}

/// Один запрос: прочитать из `reader`, передать главному циклу, ответ
/// записать в `writer`. Отдельно от сокета — тест подставляет свой писатель.
fn respond(reader: impl Read, mut writer: impl Write, tx: &Sender<Msg>) {
    let mut line = Vec::new();
    let mut reader = BufReader::new(reader.take(ipc::MAX_REQUEST));
    if reader.read_until(b'\n', &mut line).is_err() {
        return;
    }
    let request = serde_json::from_slice::<Request>(&line);
    let quit = matches!(request, Ok(Request::Quit));
    let response = match request {
        Ok(req) => {
            debug!("IPC: {req:?}");
            let (reply, answer) = mpsc::channel();
            let msg = Msg::Ipc {
                req,
                deadline: Instant::now() + ipc::QUEUE_LIMIT,
                reply,
            };
            if tx.send(msg).is_err() {
                return;
            }
            // Пока служба печатает исправление, запрос ждёт очереди.
            // Опоздавший запрос главный цикл пропустит, бросив `reply`, —
            // recv_timeout тогда вернётся сразу.
            answer
                .recv_timeout(ipc::REPLY_WAIT)
                .unwrap_or_else(|_| Response::error("служба занята"))
        }
        Err(err) => Response::error(format!("непонятный запрос: {err}")),
    };
    if let Ok(mut text) = serde_json::to_string(&response) {
        text.push('\n');
        let _ = writer.write_all(text.as_bytes());
    }
    // На quit главный цикл только отвечает: выйди он сразу, с ним вышел бы и
    // процесс, и ответ мог бы не уйти. Выход — когда ответ уже записан.
    if quit && response.ok {
        let _ = tx.send(Msg::Quit);
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::path::Path;
    use std::sync::mpsc::Receiver;

    use super::*;
    use crate::daemon::Daemon;
    use crate::daemon::tests::daemon;
    use crate::ipc::request_at;
    use crate::testutil::TempDir;

    /// Служба без устройств с сокетом во временном каталоге; главный цикл
    /// крутит сам тест — `ask`.
    struct Service {
        socket: Socket,
        daemon: Daemon,
        rx: Receiver<Msg>,
    }

    impl Service {
        fn start(dir: &TempDir) -> Service {
            let (tx, rx) = mpsc::channel();
            Service {
                socket: serve(dir.join("shiftfix.sock"), tx).unwrap(),
                daemon: daemon(dir.join("config.toml")),
                rx,
            }
        }

        /// Запрос из другого потока; этот поток тем временем — главный цикл.
        fn ask(&mut self, req: Request) -> Result<Response> {
            let path = self.socket.0.clone();
            let client = thread::spawn(move || request_at(&path, &req));
            while !client.is_finished() {
                if let Ok(msg) = self.rx.recv_timeout(Duration::from_millis(10)) {
                    self.daemon.handle(msg);
                }
            }
            client.join().unwrap()
        }
    }

    /// Послать строку как есть и прочитать ответ.
    fn raw(path: &Path, line: &[u8]) -> Response {
        let mut stream = UnixStream::connect(path).unwrap();
        stream.write_all(line).unwrap();
        let mut answer = String::new();
        BufReader::new(stream).read_line(&mut answer).unwrap();
        serde_json::from_str(&answer).unwrap()
    }

    #[test]
    fn socket_answers_requests() {
        let dir = TempDir::new("ipc");
        let mut service = Service::start(&dir);
        let path = service.socket.0.clone();

        let status = service.ask(Request::Status).unwrap().status.unwrap();
        assert_eq!(status.layouts.codes, ["us", "ru"]);
        assert!(status.enabled);
        assert!(service.ask(Request::Toggle).unwrap().ok);
        assert!(!service.daemon.engine.enabled());

        // Верный запрос, но после MAX_REQUEST пробелов: сервер его не дочитывает.
        let long = format!(
            "{}{{\"cmd\":\"status\"}}\n",
            " ".repeat(ipc::MAX_REQUEST as usize)
        );
        for line in [
            &b"{\"cmd\":\"fly\"}\n"[..],
            "не json\n".as_bytes(),
            b"\xd0\n",
            b"\n",
            long.as_bytes(),
        ] {
            let resp = raw(&path, line);
            assert!(!resp.ok);
            assert!(resp.error.unwrap().starts_with("непонятный запрос"));
        }

        drop(service);
        assert!(!path.exists(), "сторож удаляет файл сокета");
    }

    #[test]
    fn late_request_is_not_executed() {
        let dir = TempDir::new("ipc");
        let mut service = Service::start(&dir);
        // Главный цикл занят: запрос ждёт дольше срока, клиент получает отказ…
        let path = service.socket.0.clone();
        let started = Instant::now();
        let err = request_at(&path, &Request::Toggle).unwrap_err();
        assert_eq!(err.to_string(), "служба занята");
        assert!(started.elapsed() < ipc::TIMEOUT);
        // …и когда цикл освободится, запрос уже не выполняется.
        for msg in service.rx.try_iter() {
            assert!(service.daemon.handle(msg));
        }
        assert!(service.daemon.engine.enabled(), "пауза не включилась");
        let status = service.ask(Request::Status).unwrap().status.unwrap();
        assert!(status.enabled);
    }

    /// Писатель, который сообщает о записи и ждёт разрешения её закончить.
    struct Gate {
        writing: Sender<()>,
        go: Receiver<()>,
    }

    impl Write for Gate {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let _ = self.writing.send(());
            let _ = self.go.recv();
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn quit_is_answered_before_exit() {
        let mut daemon = daemon(std::env::temp_dir().join("shiftfix-none.toml"));
        let (tx, rx) = mpsc::channel();
        let (writing, written) = mpsc::channel();
        let (go, wait) = mpsc::channel();
        let gate = Gate { writing, go: wait };
        let server = thread::spawn(move || respond(&b"{\"cmd\":\"quit\"}\n"[..], gate, &tx));
        // Главный цикл отвечает на quit, но не выходит…
        let msg = rx.recv_timeout(ipc::TIMEOUT).unwrap();
        assert!(matches!(
            msg,
            Msg::Ipc {
                req: Request::Quit,
                ..
            }
        ));
        assert!(daemon.handle(msg));
        // …пока ответ пишется, сообщения о выходе нет…
        written.recv_timeout(ipc::TIMEOUT).unwrap();
        assert!(rx.try_recv().is_err(), "выход раньше ответа");
        // …а записан — главный цикл выходит.
        go.send(()).unwrap();
        let msg = rx.recv_timeout(ipc::TIMEOUT).unwrap();
        assert!(!daemon.handle(msg));
        server.join().unwrap();

        // Через сокет: ответ ok.
        let dir = TempDir::new("ipc");
        let mut service = Service::start(&dir);
        assert!(service.ask(Request::Quit).unwrap().ok);
    }
}
