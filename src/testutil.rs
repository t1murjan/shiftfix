//! Общее для модульных тестов.

use std::fs;
use std::ops::Deref;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::Result;
use evdev::KeyCode;

use crate::engine::{Keys, Stroke};

/// Эмуляция нажатий, которая ничего не делает.
pub struct NoKeys;

impl Keys for NoKeys {
    fn backspace(&mut self, _: usize) -> Result<()> {
        Ok(())
    }

    fn type_strokes(&mut self, _: &[Stroke]) -> Result<()> {
        Ok(())
    }

    fn combo(&mut self, _: &[KeyCode]) -> Result<()> {
        Ok(())
    }
}

/// Пустой временный каталог `shiftfix-<tag>-<pid>-<n>`, свой у каждого вызова:
/// тесты идут параллельно. Удаляется в Drop, в том числе при панике.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> TempDir {
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("shiftfix-{tag}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir); // остаток прогона с тем же pid
        fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }
}

impl Deref for TempDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Исполняемый скрипт `dir/name`: `#!/bin/sh` и `body`. Возвращает путь к нему.
pub fn write_script(dir: &Path, name: &str, body: &str) -> String {
    let path = dir.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path.to_string_lossy().into_owned()
}

/// Временный исполняемый скрипт вместо внешней команды (hyprctl, swaymsg,
/// gsettings…): каталог со скриптом и путь к нему.
pub fn fake_command(name: &str, body: &str) -> (TempDir, String) {
    let dir = TempDir::new(name);
    let path = write_script(&dir, name, body);
    (dir, path)
}
