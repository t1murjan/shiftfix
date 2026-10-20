//! Один экземпляр окна и один — попапа: у каждого своя блокировка и свой
//! сокет в XDG_RUNTIME_DIR. Второй запуск окна просит открытое окно
//! показаться и открыть вкладку (строка `show` и имя вкладки), второй запуск
//! попапа — закрыться (`close`): так щелчок по значку в трее и открывает, и
//! закрывает попап.

use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use shiftfix::sys::{lock, paths};

use crate::app::Page;

/// Просьба к попапу закрыться.
pub const CLOSE: &str = "close";

#[derive(Clone, Copy)]
pub enum Instance {
    /// Окно с вкладками (`shiftfix-gui`).
    Window,
    /// Попап у значка в трее (`shiftfix-gui --popup`).
    Popup,
}

/// Итог попытки запуститься единственным экземпляром.
pub enum Start {
    /// Других нет. Блокировка действует, пока открыт файл; `None` — файл
    /// блокировки не открылся, и экземпляр работает без неё.
    First(Option<File>),
    /// Экземпляр уже запущен.
    Second,
}

impl Instance {
    /// Файл экземпляра в XDG_RUNTIME_DIR: `lock` — блокировка, `sock` — сокет.
    fn path(self, extension: &str) -> PathBuf {
        let name = match self {
            Instance::Window => "shiftfix-gui",
            Instance::Popup => "shiftfix-popup",
        };
        paths::runtime_dir().join(format!("{name}.{extension}"))
    }

    /// Взять блокировку экземпляра.
    pub fn start(self) -> Start {
        start_at(&self.path("lock"))
    }

    /// Передать строку запущенному экземпляру; `false` — он не ответил.
    pub fn send(self, line: &str) -> bool {
        send_to(&self.path("sock"), line)
    }

    /// Слушать сокет экземпляра в своём потоке: каждую строку — в `handle`.
    pub fn listen(self, handle: impl FnMut(&str) + Send + 'static) {
        listen_at(self.path("sock"), handle);
    }
}

fn start_at(path: &Path) -> Start {
    match lock::try_lock(path) {
        Ok(Some(file)) => Start::First(Some(file)),
        Ok(None) => Start::Second,
        Err(err) => {
            log::warn!("блокировка {}: {err} — открываюсь без неё", path.display());
            Start::First(None)
        }
    }
}

fn send_to(path: &Path, line: &str) -> bool {
    UnixStream::connect(path)
        .and_then(|mut stream| stream.write_all(format!("{line}\n").as_bytes()))
        .is_ok()
}

fn listen_at(path: PathBuf, mut handle: impl FnMut(&str) + Send + 'static) {
    let _ = fs::remove_file(&path); // блокировка у нас — старый сокет ничей
    let listener = match UnixListener::bind(&path) {
        Ok(listener) => listener,
        Err(err) => {
            log::warn!(
                "сокет {}: {err} — повторный запуск сюда не достучится",
                path.display()
            );
            return;
        }
    };
    let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    let spawned = thread::Builder::new()
        .name("instance".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                let mut line = String::new();
                if BufReader::new(stream).read_line(&mut line).is_ok() {
                    handle(line.trim_end());
                }
            }
        });
    if let Err(err) = spawned {
        log::warn!("сокет {}: поток не запущен: {err}", path.display());
    }
}

/// Просьба к окну показаться: `show` и, если вкладку назвали в `--page`, её имя.
pub fn raise_line(page: Option<Page>) -> String {
    match page {
        Some(page) => format!("show {}", page.name()),
        None => "show".to_owned(),
    }
}

/// Разбор просьбы к окну: `None` — это не она, `Some(None)` — показаться,
/// не меняя вкладку. Незнакомая вкладка (от другой версии) — тоже просто показаться.
pub fn parse_raise(line: &str) -> Option<Option<Page>> {
    let mut words = line.split_whitespace();
    if words.next() != Some("show") {
        return None;
    }
    Some(words.next().and_then(Page::parse))
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    #[test]
    fn second_popup_closes_first() {
        // Путь сокета — не длиннее 107 байт: при длинном TMPDIR — /tmp.
        let base = Some(std::env::temp_dir())
            .filter(|t| t.as_os_str().len() <= 60)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let dir = base.join(format!("shiftfix-instance-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let (lock, socket) = (dir.join("popup.lock"), dir.join("popup.sock"));

        // Первый попап: блокировка свободна, он слушает сокет.
        let Start::First(Some(_held)) = start_at(&lock) else {
            panic!("первый запуск не взял блокировку");
        };
        let (lines, received) = mpsc::channel();
        listen_at(socket.clone(), move |line| {
            let _ = lines.send(line.to_owned());
        });
        // Второй: блокировка занята — он просит первый закрыться.
        assert!(matches!(start_at(&lock), Start::Second));
        assert!(send_to(&socket, CLOSE));
        let line = received.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(line, CLOSE);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn raise_carries_page() {
        for page in Page::ALL {
            assert_eq!(parse_raise(&raise_line(Some(page))), Some(Some(page)));
        }
        assert_eq!(parse_raise(&raise_line(None)), Some(None));
        // Вкладка из другой версии окна — показаться, вкладку не трогать.
        assert_eq!(parse_raise("show nope\n"), Some(None));
        assert_eq!(parse_raise("hide\n"), None);
        assert_eq!(parse_raise(""), None);
    }
}
