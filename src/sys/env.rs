//! Где искать сеанс. Служба может стартовать раньше сеанса или пережить его
//! перезапуск, поэтому адреса Wayland, X11, Hyprland и Sway берутся по очереди
//! из собственного окружения, из окружения пользовательского systemd (сеанс
//! обновляет его при входе) и поиском сокетов. Годится только живой сокет:
//! после падения композитора файл остаётся, а слушать его уже некому.

use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::sys::dbus::{Bus, Client};
use crate::sys::paths::{non_empty, runtime_dir, uid};

/// Сколько помнить найденное: адреса нужны при каждом запуске внешней команды.
const CACHE_TTL: Duration = Duration::from_secs(2);

type Slot<T> = Mutex<Option<(Instant, T)>>;

fn cached<T: Clone>(slot: &Slot<T>, compute: impl FnOnce() -> T) -> T {
    let mut guard = slot.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, value)) = guard.as_ref()
        && at.elapsed() < CACHE_TTL
    {
        return value.clone();
    }
    let value = compute();
    *guard = Some((Instant::now(), value.clone()));
    value
}

/// Окружение пользовательского systemd: туда сеанс экспортирует WAYLAND_DISPLAY,
/// DISPLAY, XAUTHORITY и прочее при входе.
fn manager_env() -> HashMap<String, String> {
    static SLOT: Slot<HashMap<String, String>> = Mutex::new(None);
    static BUS: Mutex<Client> = Mutex::new(Client::new(Bus::Session));
    cached(&SLOT, || {
        let mut bus = BUS.lock().unwrap_or_else(|e| e.into_inner());
        bus.property::<Vec<String>>(
            "org.freedesktop.systemd1",
            "/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager",
            "Environment",
        )
        .unwrap_or_default()
        .into_iter()
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            Some((key.to_owned(), value.to_owned()))
        })
        .collect()
    })
}

/// Переменная сеанса: своя или из окружения systemd.
pub(crate) fn session_var(name: &str) -> Option<String> {
    non_empty(name).or_else(|| manager_env().get(name).filter(|v| !v.is_empty()).cloned())
}

/// Свои и systemd-значения переменной — кандидаты по порядку.
fn candidates(name: &str) -> Vec<String> {
    let mut list: Vec<String> = non_empty(name).into_iter().collect();
    if let Some(value) = manager_env().get(name)
        && !value.is_empty()
        && !list.contains(value)
    {
        list.push(value.clone());
    }
    list
}

fn is_socket(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.file_type().is_socket())
}

/// Сокет слушают (а не просто остался файл).
fn alive(path: &Path) -> bool {
    UnixStream::connect(path).is_ok()
}

fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}

/// Путь к сокету Wayland по `WAYLAND_DISPLAY`.
pub(crate) fn wayland_path(display: &str) -> PathBuf {
    if display.starts_with('/') {
        display.into()
    } else {
        runtime_dir().join(display)
    }
}

/// Живой сокет Wayland: `WAYLAND_DISPLAY` или первый работающий `wayland-N`.
pub fn wayland_display() -> Option<String> {
    static SLOT: Slot<Option<String>> = Mutex::new(None);
    cached(&SLOT, || {
        let dir = runtime_dir();
        let scanned = names_in(&dir).into_iter().filter(|n| {
            n.starts_with("wayland-") && !n.ends_with(".lock") && is_socket(&dir.join(n))
        });
        candidates("WAYLAND_DISPLAY")
            .into_iter()
            .chain(scanned)
            .find(|d| alive(&wayland_path(d)))
    })
}

/// Подпись запущенного Hyprland (каталог с его сокетом).
pub(crate) fn hyprland_signature() -> Option<String> {
    static SLOT: Slot<Option<String>> = Mutex::new(None);
    cached(&SLOT, || {
        let bases = [runtime_dir().join("hypr"), PathBuf::from("/tmp/hypr")];
        let socket = |sig: &str| {
            bases
                .iter()
                .map(|b| b.join(sig).join(".socket.sock"))
                .find(|p| is_socket(p))
        };
        let mut found: Vec<(std::time::SystemTime, String)> = bases
            .iter()
            .flat_map(|base| fs::read_dir(base).into_iter().flatten().flatten())
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                let modified = e.metadata().and_then(|m| m.modified()).ok()?;
                Some((modified, name))
            })
            .collect();
        found.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified)); // сначала самый свежий
        candidates("HYPRLAND_INSTANCE_SIGNATURE")
            .into_iter()
            .chain(found.into_iter().map(|(_, name)| name))
            .find(|sig| socket(sig).is_some_and(|p| alive(&p)))
    })
}

/// Живой сокет IPC Sway.
pub(crate) fn sway_socket() -> Option<String> {
    static SLOT: Slot<Option<String>> = Mutex::new(None);
    cached(&SLOT, || {
        let dir = runtime_dir();
        let scanned = names_in(&dir)
            .into_iter()
            .filter(|n| n.starts_with("sway-ipc.") && n.ends_with(".sock"))
            .map(|n| dir.join(n).to_string_lossy().into_owned());
        candidates("SWAYSOCK")
            .into_iter()
            .chain(scanned)
            .find(|s| alive(Path::new(s)))
    })
}

/// Сокет X-сервера с этим номером.
pub(crate) fn x11_socket(number: u32) -> String {
    format!("/tmp/.X11-unix/X{number}")
}

/// Номер дисплея из `:0`, `:1.0`, `unix/:0`.
pub(crate) fn x11_number(display: &str) -> Option<u32> {
    let rest = display.rsplit_once(':')?.1;
    rest.split('.').next()?.parse().ok()
}

/// Жив ли X-сервер с этим номером. Без подключения: в GNOME подключение
/// к :0 запускает XWayland «по требованию».
fn x11_alive(number: u32) -> bool {
    let pid = fs::read_to_string(format!("/tmp/.X{number}-lock"))
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok());
    match pid {
        Some(pid) => Path::new(&format!("/proc/{pid}")).exists(),
        None => is_socket(Path::new(&x11_socket(number))),
    }
}

/// Дисплей X11 (сеанс X11 или XWayland): `:N`.
pub fn x11_display() -> Option<String> {
    static SLOT: Slot<Option<String>> = Mutex::new(None);
    cached(&SLOT, || {
        let own = |n: &u32| fs::metadata(x11_socket(*n)).is_ok_and(|m| m.uid() == uid());
        let mut scanned: Vec<u32> = names_in(Path::new("/tmp/.X11-unix"))
            .iter()
            .filter_map(|n| n.strip_prefix('X')?.parse().ok())
            .collect();
        // Сначала свои (XWayland, Xorg без root), потом чужие.
        scanned.sort_by_key(|n| (!own(n), *n));
        candidates("DISPLAY")
            .into_iter()
            .filter(|d| x11_number(d).is_some())
            .chain(scanned.into_iter().map(|n| format!(":{n}")))
            .find(|d| x11_number(d).is_some_and(x11_alive))
    })
}

/// Графический сеанс есть хоть какой-нибудь.
pub fn graphical_session() -> bool {
    wayland_display().is_some() || x11_display().is_some()
}

/// Рабочий стол в нижнем регистре (`kde`, `gnome`, `hyprland`…), если известен.
pub fn desktop() -> String {
    session_var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;

    use super::*;

    #[test]
    fn parses_display_numbers() {
        assert_eq!(x11_number(":0"), Some(0));
        assert_eq!(x11_number(":1.0"), Some(1));
        assert_eq!(x11_number("unix/:12"), Some(12));
        assert_eq!(x11_number("wayland-0"), None);
        assert_eq!(x11_number(""), None);
    }

    #[test]
    fn dead_socket_is_not_alive() {
        let dir = crate::testutil::TempDir::new("env");
        let path = dir.join("wayland-9");
        let listener = UnixListener::bind(&path).unwrap();
        assert!(is_socket(&path) && alive(&path));
        drop(listener); // композитор упал, файл остался
        // Копия сокета ещё может жить у процесса, которого как раз запускает
        // соседний тест (fork до exec), — подождать, пока она закроется.
        let started = Instant::now();
        while alive(&path) && started.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(is_socket(&path) && !alive(&path));
    }
}
