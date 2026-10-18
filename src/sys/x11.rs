//! Подключение к X-серверу сеанса: X11 (Mint/Cinnamon, XFCE, MATE…) или
//! XWayland — через него выделение отдаёт GNOME, у которого нет data-control.
//!
//! Подключение своё (x11rb, без libX11): служба часто стартует раньше сеанса,
//! и XAUTHORITY приходится искать самим.

use std::cmp::Reverse;
use std::fs;
use std::os::linux::net::SocketAddrExt;
use std::os::unix::net::{SocketAddr, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use anyhow::{Context, Result, anyhow, bail};
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _};
use x11rb::rust_connection::{DefaultStream, RustConnection};

use crate::sys::{env, paths};

pub struct XConn {
    pub conn: RustConnection,
    pub root: u32,
}

static X_AUTHORITY: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Файл Xauthority, с которым удалось подключиться (его же получат xclip и GUI).
fn set_x_authority(path: Option<PathBuf>) {
    *X_AUTHORITY.lock().unwrap_or_else(|e| e.into_inner()) = path;
}

pub fn x_authority() -> Option<PathBuf> {
    X_AUTHORITY
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .or_else(|| env::session_var("XAUTHORITY").map(PathBuf::from))
}

/// Куки MIT-MAGIC-COOKIE-1 для дисплея из файла Xauthority.
fn parse_xauth(data: &[u8], number: u32) -> Vec<Vec<u8>> {
    fn take<'a>(data: &'a [u8], pos: &mut usize, n: usize) -> Option<&'a [u8]> {
        let slice = data.get(*pos..pos.checked_add(n)?)?;
        *pos += n;
        Some(slice)
    }
    fn field<'a>(data: &'a [u8], pos: &mut usize) -> Option<&'a [u8]> {
        let len = take(data, pos, 2)?;
        let len = usize::from(u16::from_be_bytes([len[0], len[1]]));
        take(data, pos, len)
    }
    let wanted = number.to_string();
    let mut pos = 0;
    let mut cookies = Vec::new();
    while pos < data.len() {
        // семейство адреса, адрес, номер дисплея, метод, данные
        let Some(_family) = take(data, &mut pos, 2) else {
            break;
        };
        let (Some(_address), Some(display), Some(name), Some(cookie)) = (
            field(data, &mut pos),
            field(data, &mut pos),
            field(data, &mut pos),
            field(data, &mut pos),
        ) else {
            break;
        };
        let matches = display.is_empty() || display == wanted.as_bytes();
        if matches && name == b"MIT-MAGIC-COOKIE-1" && !cookies.iter().any(|c| c == cookie) {
            cookies.push(cookie.to_vec());
        }
    }
    cookies
}

/// Где может лежать Xauthority: KDE и SDDM, GNOME (XWayland и GDM), домашний.
fn auth_files() -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = env::session_var("XAUTHORITY")
        .map(PathBuf::from)
        .into_iter()
        .collect();
    if let Some(home) = std::env::var_os("HOME") {
        files.push(PathBuf::from(home).join(".Xauthority"));
    }
    let run = paths::runtime_dir();
    files.extend(session_auth_files(&run));
    files.push(run.join("gdm").join("Xauthority"));
    let mut unique = Vec::new();
    for file in files {
        if file.is_file() && !unique.contains(&file) {
            unique.push(file);
        }
    }
    unique
}

/// Файлы Xauthority сеансов KDE и GNOME, сначала самый свежий: от прошлых
/// сеансов остаются старые.
fn session_auth_files(run: &Path) -> Vec<PathBuf> {
    let mut found: Vec<(Reverse<Option<SystemTime>>, PathBuf)> = fs::read_dir(run)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.starts_with("xauth_") || name.starts_with(".mutter-Xwaylandauth.")
        })
        .map(|e| {
            let modified = e.metadata().and_then(|m| m.modified()).ok();
            (Reverse(modified), e.path())
        })
        .collect();
    found.sort();
    found.into_iter().map(|(_, path)| path).collect()
}

/// Сколько подключений пробовать: каждое — круг до X-сервера.
const ATTEMPTS: usize = 12;

/// Куки из файлов по порядку, без повторов; последняя попытка — без
/// авторизации (XWayland в wlroots и др.).
fn attempts(files: &[(PathBuf, Vec<u8>)], number: u32) -> Vec<(Option<PathBuf>, Option<Vec<u8>>)> {
    let mut attempts: Vec<(Option<PathBuf>, Option<Vec<u8>>)> = Vec::new();
    for (file, data) in files {
        for cookie in parse_xauth(data, number) {
            if !attempts.iter().any(|(_, c)| c.as_ref() == Some(&cookie)) {
                attempts.push((Some(file.clone()), Some(cookie)));
            }
        }
    }
    attempts.truncate(ATTEMPTS - 1);
    attempts.push((None, None));
    attempts
}

fn stream(number: u32) -> std::io::Result<UnixStream> {
    let path = format!("/tmp/.X11-unix/X{number}");
    // Как libxcb: сначала абстрактный сокет, потом файл.
    SocketAddr::from_abstract_name(path.as_bytes())
        .and_then(|addr| UnixStream::connect_addr(&addr))
        .or_else(|_| UnixStream::connect(&path))
}

fn open(number: u32, cookie: Option<Vec<u8>>) -> Result<RustConnection> {
    let (stream, _) = DefaultStream::from_unix_stream(stream(number)?)?;
    let (name, data) = match cookie {
        Some(cookie) => (b"MIT-MAGIC-COOKIE-1".to_vec(), cookie),
        None => (Vec::new(), Vec::new()),
    };
    Ok(RustConnection::connect_to_stream_with_auth_info(
        stream, 0, name, data,
    )?)
}

pub fn connect() -> Result<XConn> {
    let display = env::x11_display().context("X-сервер не найден")?;
    let number = env::x11_number(&display).context("номер дисплея")?;
    let files: Vec<(PathBuf, Vec<u8>)> = auth_files()
        .into_iter()
        .map(|file| {
            let data = fs::read(&file).unwrap_or_default();
            (file, data)
        })
        .collect();
    let mut last = anyhow!("нет подходящего Xauthority");
    for (file, cookie) in attempts(&files, number) {
        match open(number, cookie) {
            Ok(conn) => {
                if file.is_some() {
                    set_x_authority(file);
                }
                let root = conn
                    .setup()
                    .roots
                    .first()
                    .context("у X-сервера нет экранов")?
                    .root;
                return Ok(XConn { conn, root });
            }
            Err(err) => last = err,
        }
    }
    Err(last.context(format!("X-сервер {display}")))
}

/// Свойство `_XKB_RULES_NAMES` корневого окна: из чего X-сервер собрал keymap.
/// Раскладки и варианты — через запятую: `us,ru` и `,phonetic`.
#[derive(Debug, Default, PartialEq)]
pub struct RulesNames {
    pub rules: String,
    pub model: String,
    pub layout: String,
    pub variant: String,
    pub options: String,
}

impl RulesNames {
    /// Значение свойства — строки через нулевой байт; недостающие пустые.
    fn parse(value: &[u8]) -> RulesNames {
        let mut parts = value
            .split(|b| *b == 0)
            .map(|part| String::from_utf8_lossy(part).into_owned());
        let mut next = || parts.next().unwrap_or_default();
        RulesNames {
            rules: next(),
            model: next(),
            layout: next(),
            variant: next(),
            options: next(),
        }
    }
}

pub fn rules_names(x: &XConn) -> Result<RulesNames> {
    let atom = x
        .conn
        .intern_atom(false, b"_XKB_RULES_NAMES")?
        .reply()?
        .atom;
    let reply = x
        .conn
        .get_property(false, x.root, atom, AtomEnum::STRING, 0, 4096)?
        .reply()?;
    if reply.value.is_empty() {
        bail!("X-сервер не сообщает раскладки (_XKB_RULES_NAMES)");
    }
    Ok(RulesNames::parse(&reply.value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(display: &str, name: &[u8], cookie: &[u8]) -> Vec<u8> {
        let mut out = vec![1, 0]; // FamilyLocal
        for field in [b"host".as_slice(), display.as_bytes(), name, cookie] {
            out.extend(u16::try_from(field.len()).unwrap().to_be_bytes());
            out.extend(field);
        }
        out
    }

    #[test]
    fn parses_rules_names() {
        let names = RulesNames::parse(b"evdev\0pc105\0us,ru\0,phonetic\0grp:caps_toggle\0");
        assert_eq!(
            names,
            RulesNames {
                rules: "evdev".into(),
                model: "pc105".into(),
                layout: "us,ru".into(),
                variant: ",phonetic".into(),
                options: "grp:caps_toggle".into(),
            }
        );
        let short = RulesNames::parse(b"evdev\0pc105\0us");
        assert_eq!((short.layout.as_str(), short.options.as_str()), ("us", ""));
    }

    #[test]
    fn parses_xauthority() {
        let mut file = record("0", b"MIT-MAGIC-COOKIE-1", &[1, 2, 3]);
        file.extend(record("1", b"MIT-MAGIC-COOKIE-1", &[4, 5, 6]));
        file.extend(record("", b"MIT-MAGIC-COOKIE-1", &[7]));
        file.extend(record("1", b"XDM-AUTHORIZATION-1", &[8]));
        assert_eq!(parse_xauth(&file, 1), [vec![4, 5, 6], vec![7]]);
        assert_eq!(parse_xauth(&file, 0), [vec![1, 2, 3], vec![7]]);
        // Обрезанный файл не роняет разбор.
        assert_eq!(parse_xauth(&file[..5], 0), Vec::<Vec<u8>>::new());
    }

    #[test]
    fn always_tries_without_auth() {
        let cookies: Vec<u8> = (0..20)
            .flat_map(|i| record("0", b"MIT-MAGIC-COOKIE-1", &[i]))
            .collect();
        let list = attempts(&[(PathBuf::from("a"), cookies)], 0);
        assert_eq!(list.len(), ATTEMPTS);
        assert_eq!(list[0], (Some(PathBuf::from("a")), Some(vec![0])));
        assert_eq!(list.last(), Some(&(None, None)));
    }

    #[test]
    fn fresh_auth_files_first() {
        let dir = crate::testutil::TempDir::new("xauth");
        let now = SystemTime::now();
        for (name, age) in [("xauth_a", 60), ("xauth_b", 0), ("other", 0)] {
            let file = fs::File::create(dir.join(name)).unwrap();
            file.set_modified(now - std::time::Duration::from_secs(age))
                .unwrap();
        }
        assert_eq!(
            session_auth_files(&dir),
            [dir.join("xauth_b"), dir.join("xauth_a")]
        );
    }
}
