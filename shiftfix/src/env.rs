//! Где искать сессию. Служба systemd может стартовать раньше, чем сессия
//! экспортирует WAYLAND_DISPLAY и прочие переменные, поэтому сокеты Wayland,
//! Hyprland и Sway ищутся и напрямую в XDG_RUNTIME_DIR.

use std::env;
use std::fs;
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn non_empty(var: &str) -> Option<String> {
    env::var(var).ok().filter(|v| !v.is_empty())
}

pub fn runtime_dir() -> PathBuf {
    if let Some(dir) = non_empty("XDG_RUNTIME_DIR") {
        return dir.into();
    }
    // SAFETY: getuid не имеет ошибок и побочных эффектов.
    let uid = unsafe { libc::getuid() };
    let run = PathBuf::from(format!("/run/user/{uid}"));
    if run.is_dir() { run } else { env::temp_dir() }
}

fn is_socket(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.file_type().is_socket())
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

/// `WAYLAND_DISPLAY` или первый живой сокет `wayland-N`.
pub fn wayland_display() -> Option<String> {
    non_empty("WAYLAND_DISPLAY").or_else(|| {
        let dir = runtime_dir();
        names_in(&dir)
            .into_iter()
            .find(|n| n.starts_with("wayland-") && !n.ends_with(".lock") && is_socket(&dir.join(n)))
    })
}

/// Подпись запущенного Hyprland (каталог с его сокетом).
pub fn hyprland_signature() -> Option<String> {
    if let Some(sig) = non_empty("HYPRLAND_INSTANCE_SIGNATURE") {
        return Some(sig);
    }
    for base in [runtime_dir().join("hypr"), PathBuf::from("/tmp/hypr")] {
        let newest = fs::read_dir(&base)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| is_socket(&e.path().join(".socket.sock")))
            .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok());
        if let Some(entry) = newest {
            return entry.file_name().into_string().ok();
        }
    }
    None
}

/// Сокет IPC Sway.
pub fn sway_socket() -> Option<String> {
    non_empty("SWAYSOCK").or_else(|| {
        let dir = runtime_dir();
        names_in(&dir)
            .into_iter()
            .find(|n| n.starts_with("sway-ipc.") && n.ends_with(".sock") && is_socket(&dir.join(n)))
            .map(|n| dir.join(n).to_string_lossy().into_owned())
    })
}

/// Команда с переменными сессии: wl-paste, hyprctl, swaymsg и GUI найдут композитор,
/// даже если сама служба запустилась раньше сессии.
pub fn session_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut cmd = Command::new(program);
    if env::var_os("XDG_RUNTIME_DIR").is_none() {
        cmd.env("XDG_RUNTIME_DIR", runtime_dir());
    }
    if let Some(display) = wayland_display() {
        cmd.env("WAYLAND_DISPLAY", display);
    }
    if let Some(sig) = hyprland_signature() {
        cmd.env("HYPRLAND_INSTANCE_SIGNATURE", sig);
    }
    if let Some(sock) = sway_socket() {
        cmd.env("SWAYSOCK", sock);
    }
    cmd
}

/// Есть ли программа в PATH.
pub fn in_path(tool: &str) -> bool {
    env::var_os("PATH")
        .is_some_and(|path| env::split_paths(&path).any(|dir| dir.join(tool).is_file()))
}

/// Соседний исполняемый файл (shiftfix ↔ shiftfix-gui), иначе — из PATH.
pub fn sibling_binary(name: &str) -> PathBuf {
    env::current_exe()
        .ok()
        .map(|exe| exe.with_file_name(name))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from(name))
}
