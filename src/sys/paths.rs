//! Свои пути и программы: uid, каталоги настроек и сокетов, поиск программ
//! в PATH и рядом с собой.

use std::env;
use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

pub(super) fn non_empty(var: &str) -> Option<String> {
    env::var(var).ok().filter(|v| !v.is_empty())
}

pub fn uid() -> u32 {
    // SAFETY: getuid не имеет ошибок и побочных эффектов.
    unsafe { libc::getuid() }
}

/// Домашний каталог (`$HOME`).
pub fn home() -> Option<PathBuf> {
    env::var_os("HOME").map(PathBuf::from)
}

/// Каталог настроек: `$XDG_CONFIG_HOME`, иначе `~/.config`.
pub fn config_home() -> PathBuf {
    env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| home().map(|home| home.join(".config")))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Каталог сокетов и блокировок: `$XDG_RUNTIME_DIR`, иначе `/run/user/UID`,
/// иначе свой каталог во временном.
pub fn runtime_dir() -> PathBuf {
    if let Some(dir) = non_empty("XDG_RUNTIME_DIR") {
        return dir.into();
    }
    let run = PathBuf::from(format!("/run/user/{}", uid()));
    if run.is_dir() {
        run
    } else {
        private_dir(&env::temp_dir())
    }
}

/// В общем `/tmp` блокировка и сокет службы другого пользователя мешали бы
/// нашим, поэтому — `shiftfix-UID` с доступом только себе.
fn private_dir(tmp: &Path) -> PathBuf {
    let dir = tmp.join(format!("shiftfix-{}", uid()));
    let _ = fs::DirBuilder::new().mode(0o700).create(&dir);
    dir
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

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn private_runtime_fallback() {
        let tmp = crate::testutil::TempDir::new("runtime");
        let dir = private_dir(&tmp);
        assert_eq!(dir, tmp.join(format!("shiftfix-{}", uid())));
        assert_eq!(private_dir(&tmp), dir);
        let meta = fs::metadata(&dir).unwrap();
        assert!(meta.is_dir());
        assert_eq!(meta.permissions().mode() & 0o777, 0o700);
    }
}
