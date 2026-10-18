//! Переключение раскладки: KWin по DBus, GNOME, Hyprland, Sway, XKB в X11
//! или эмуляция хоткея.
//! В режиме «авто» окружение определяется при первом обращении и заново —
//! если оно сменилось или служба стартовала раньше сессии.

mod auto;
mod gnome;
mod hotkey;
mod hyprland;
mod kde;
mod sway;
mod x11;

pub use auto::Auto;
pub use gnome::Gnome;
pub use hotkey::Hotkey;
pub use hyprland::Hyprland;
pub use kde::Kde;
pub use sway::Sway;
pub use x11::XkbLayout;

use anyhow::{Context, Result};
use serde_json::Value;

use crate::config::{BackendKind, Config};
use crate::engine::Backend;
use crate::sys::proc::run_text;

/// Переключатель, выбранный в конфиге (`backend = …`).
pub fn make(cfg: &Config) -> Box<dyn Backend> {
    let hotkey = || Hotkey::new(cfg.switch_hotkey.clone(), cfg.hotkey_layouts.clone());
    match cfg.backend {
        BackendKind::Auto => Box::new(Auto::new(hotkey())),
        BackendKind::Kde => Box::new(Kde::default()),
        BackendKind::Gnome => Box::new(Gnome::new(cfg.switch_hotkey.clone())),
        BackendKind::Hyprland => Box::new(Hyprland::default()),
        BackendKind::Sway => Box::new(Sway::default()),
        BackendKind::X11 => Box::new(XkbLayout::default()),
        BackendKind::Hotkey => Box::new(hotkey()),
    }
}

/// Ответ внешней команды в JSON (`hyprctl -j`, `swaymsg -r`).
fn run_json(cmd: &str, args: &[&str]) -> Result<Value> {
    let out = run_text(cmd, args)?;
    serde_json::from_str(&out).with_context(|| format!("{cmd}: неожиданный ответ"))
}

/// Виртуальная клавиатура shiftfix в списке клавиатур композитора: через неё
/// перенабирается слово, а раскладку определяет настоящая.
fn is_ours(keyboard: &Value) -> bool {
    keyboard["name"]
        .as_str()
        .unwrap_or_default()
        .contains("shiftfix")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_falls_back_to_hotkey() {
        let cfg = Config::default();
        let mut auto = make(&cfg);
        let layouts = auto.layouts(); // здесь Auto ищет окружение
        if auto.name() != Hotkey::new(Vec::new(), Vec::new()).name() {
            // На рабочем столе (KDE, Hyprland, X11…) проверять нечего. Это не
            // нехватка программ — пропуск и в строгом режиме.
            eprintln!("— пропуск: раскладку переключает {}", auto.name());
            return;
        }
        assert_eq!(layouts.unwrap(), cfg.hotkey_layouts);
        assert_eq!(auto.current().unwrap(), None);
    }
}
