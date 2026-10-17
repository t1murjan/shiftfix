//! Строки для показа в окне: путь с «~», время, сравнение версий.

use std::path::{Path, PathBuf};

/// Служба другой версии, чем окно: после обновления её не перезапустили.
/// Пустая версия службы (не сообщила) — не расхождение.
pub fn versions_differ(window: &str, service: &str) -> bool {
    let norm = |v: &str| v.trim().trim_start_matches('v').to_owned();
    let service = norm(service);
    !service.is_empty() && service != norm(window)
}

/// Путь для показа: домашний каталог — «~».
pub fn short_path(path: &Path, home: Option<&Path>) -> String {
    match home.filter(|h| !h.as_os_str().is_empty()) {
        Some(home) => match path.strip_prefix(home) {
            Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => path.display().to_string(),
        },
        None => path.display().to_string(),
    }
}

/// Путь для показа относительно `$HOME`.
pub fn shown_path(path: &Path) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    short_path(path, home.as_deref())
}

/// Unix-время → «ЧЧ:ММ:СС» по местному времени.
pub fn clock(ts: u64) -> String {
    let t = libc::time_t::try_from(ts).unwrap_or_default();
    // SAFETY: localtime_r пишет в локальную структуру.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return String::from("--:--:--");
    }
    format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn versions_differ_only_on_real_mismatch() {
        assert!(!versions_differ("0.4.0", "0.4.0"));
        assert!(!versions_differ("0.4.0", " 0.4.0\n"));
        assert!(!versions_differ("0.4.0", "v0.4.0"));
        assert!(versions_differ("0.4.0", "0.3.0"));
        assert!(versions_differ("0.4.0", "0.4.1"));
        assert!(versions_differ("0.4.0", "0.4.0-dev"));
        // Служба версию не сообщила — не о чем предупреждать.
        assert!(!versions_differ("0.4.0", ""));
        assert!(!versions_differ("0.4.0", "  "));
    }

    #[test]
    fn short_path_replaces_home() {
        let home = Some(Path::new("/home/u"));
        let p = |s: &str| short_path(Path::new(s), home);
        assert_eq!(
            p("/home/u/.config/shiftfix/config.toml"),
            "~/.config/shiftfix/config.toml"
        );
        assert_eq!(p("/home/u"), "~");
        // Соседний каталог с тем же началом имени — не домашний.
        assert_eq!(p("/home/u2/config.toml"), "/home/u2/config.toml");
        assert_eq!(p("/etc/shiftfix.toml"), "/etc/shiftfix.toml");
        let path = Path::new("/home/u/x");
        assert_eq!(short_path(path, None), "/home/u/x");
        assert_eq!(short_path(path, Some(Path::new(""))), "/home/u/x");
    }
}
