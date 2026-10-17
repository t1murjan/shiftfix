//! Общее для модульных тестов.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

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

/// Пустой временный каталог `shiftfix-<tag>-<pid>`; прежний с тем же именем удаляется.
pub fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("shiftfix-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Временный исполняемый скрипт вместо внешней команды (hyprctl, swaymsg,
/// gsettings…): каталог со скриптом и путь к нему.
pub fn fake_command(name: &str, body: &str) -> (PathBuf, String) {
    let dir = temp_dir(name);
    let path = dir.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    (dir, path.to_string_lossy().into_owned())
}
