//! Общее для вставки через буфер обмена — и через wl-clipboard, и через xclip.
//!
//! Перед вставкой буфер обмена сохраняется и потом возвращается. Вернуть
//! можно только один формат, поэтому он выбирается с умом: пароли из
//! менеджера паролей не возвращаются вовсе (иначе они попадут в историю
//! Klipper и подобных), скопированные файлы возвращаются как файлы.

use std::thread;
use std::time::{Duration, Instant};

use crate::sys::env;

pub(super) const TEXT_TYPES: [&str; 5] = [
    "text/plain;charset=utf-8",
    "utf8_string",
    "text/plain",
    "string",
    "text",
];
pub(super) const UTF8: &str = "text/plain;charset=utf-8";
/// Отметка менеджеров паролей (KeePassXC, Bitwarden…): не сохранять в истории.
const SECRET: &str = "x-kde-passwordmanagerhint";
/// Признаки скопированных файлов (Dolphin, Nautilus, Thunar…).
const FILE_MARKERS: [&str; 3] = [
    "x-special/gnome-copied-files",
    "application/x-kde4-urilist",
    "application/x-kde-cutselection",
];

/// Что вернуть в буфер обмена после вставки.
#[derive(Debug, PartialEq, Eq)]
pub enum Restore {
    /// Буфер был пуст или в нём пароль — очистить.
    Clear,
    /// Вернуть данные этого типа.
    Type(String),
}

/// Выбрать тип для возврата по списку типов буфера обмена.
pub fn restore_plan(types: &[String], gnome: bool) -> Restore {
    let find = |wanted: &str| types.iter().find(|t| t.eq_ignore_ascii_case(wanted));
    if find(SECRET).is_some() {
        return Restore::Clear;
    }
    if FILE_MARKERS.iter().any(|m| find(m).is_some()) {
        let preferred = if gnome {
            ["x-special/gnome-copied-files", "text/uri-list"]
        } else {
            ["text/uri-list", "x-special/gnome-copied-files"]
        };
        if let Some(t) = preferred.iter().find_map(|m| find(m)) {
            return Restore::Type(t.clone());
        }
    }
    let useful = |t: &&String| {
        let t = t.to_ascii_lowercase();
        // Служебные цели X11 — не данные.
        !matches!(
            t.as_str(),
            "targets" | "timestamp" | "multiple" | "save_targets" | "delete" | "insert_selection"
        )
    };
    TEXT_TYPES
        .iter()
        .find_map(|m| find(m))
        .or_else(|| find("image/png"))
        .or_else(|| types.iter().find(useful))
        .map_or(Restore::Clear, |t| Restore::Type(t.clone()))
}

pub(super) fn is_gnome() -> bool {
    env::desktop().contains("gnome")
}

/// Подождать, пока буфер обмена действительно получит `data`: копирующая
/// программа уходит в фон раньше, чем композитор объявит новое содержимое.
pub(super) fn wait_until(read: impl Fn() -> Option<Vec<u8>>, data: &[u8]) -> bool {
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        if read().is_some_and(|got| got == data) {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    false
}

#[cfg(test)]
pub(super) mod testing {
    use anyhow::Result;
    use evdev::KeyCode;

    use crate::engine::Keys;
    use crate::keys::Stroke;

    /// «Приложение»: на Ctrl+V читает буфер обмена — файл `clip` фальшивых
    /// wl-clipboard и xclip.
    pub struct App {
        pub dir: std::path::PathBuf,
        pub pasted: Vec<String>,
    }

    impl Keys for App {
        fn backspace(&mut self, _: usize) -> Result<()> {
            Ok(())
        }
        fn type_strokes(&mut self, _: &[Stroke]) -> Result<()> {
            Ok(())
        }
        fn combo(&mut self, keys: &[KeyCode]) -> Result<()> {
            assert_eq!(keys, [KeyCode::KEY_LEFTCTRL, KeyCode::KEY_V]);
            self.pasted
                .push(std::fs::read_to_string(self.dir.join("clip"))?);
            Ok(())
        }
        fn chord(&mut self, _: &[KeyCode], _: KeyCode, _: usize) -> Result<()> {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn types(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn chooses_what_to_restore() {
        // KeePassXC: пароль не возвращаем.
        let secret = types(&[UTF8, "text/plain", "x-kde-passwordManagerHint"]);
        assert_eq!(restore_plan(&secret, false), Restore::Clear);
        // Файлы из Dolphin — как файлы, а не как текст с путями.
        let dolphin = types(&[
            "application/x-kde4-urilist",
            "text/uri-list",
            UTF8,
            "text/plain",
        ]);
        assert_eq!(
            restore_plan(&dolphin, false),
            Restore::Type("text/uri-list".into())
        );
        let nautilus = types(&["x-special/gnome-copied-files", "text/uri-list", UTF8]);
        assert_eq!(
            restore_plan(&nautilus, true),
            Restore::Type("x-special/gnome-copied-files".into())
        );
        // Ссылка из браузера — это текст.
        let link = types(&["text/x-moz-url", "text/uri-list", "text/html", UTF8]);
        assert_eq!(restore_plan(&link, false), Restore::Type(UTF8.into()));
        // X11: служебные цели пропускаются.
        let x11 = types(&["TIMESTAMP", "TARGETS", "MULTIPLE", "UTF8_STRING", "STRING"]);
        assert_eq!(
            restore_plan(&x11, false),
            Restore::Type("UTF8_STRING".into())
        );
        let image = types(&["TARGETS", "image/bmp", "image/png"]);
        assert_eq!(
            restore_plan(&image, false),
            Restore::Type("image/png".into())
        );
        assert_eq!(restore_plan(&types(&["TARGETS"]), false), Restore::Clear);
        assert_eq!(restore_plan(&[], false), Restore::Clear);
    }
}
